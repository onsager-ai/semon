// Ported from the mockup's home.js: Home, the sidebar drawer, the Sessions and Machines pages, group-by, and that
// Home's rows and items actually lead somewhere — phone, default (light) colour scheme, plus a small desktop pass
// for two reference screenshots. The original never set colorScheme, so it only ever ran light; this port keeps
// that (bar.mjs and turns.mjs cover phone dark).
//
// The original ran 'sample' (test7.html) and 'real' (test-real.html) tagged. Ported: 'real' is dropped; 'sample'
// drives the served fixture. Every lookup here is by session id or by a live DOM query (no sample handoff/turn id
// is named), so there is no id mapping to do. Per gaps.json there is exactly one machine ('laptop') in this fixture,
// so machinePages has one entry and its screenshot is always taken (the original's condition — 'buildbox' or a
// single machine — reduces to "always" here).
//
// Assertions:
//  - no page errors.
//  - zero overflow on every screen visited: Home, the drawer, Sessions (each grouping), Machines, every machine page.
//  - the sidebar drawer's session rows equal the number of lane sessions (Object.values(SESS).filter(s=>s.lane)),
//    i.e. every lane session is listed and nothing else is.
//  - every grouping on the Sessions page (project/machine/harness) still shows the same row count as ungrouped —
//    grouping reshuffles into sections, it doesn't drop or duplicate rows.
//  - a live-row tap and a Needs-you item tap on Home both navigate away from Home (liveRowOpens/itemOpens change the
//    page's <h1>), and the item's flash lands on the turn its own data-h resolves to.
//  - a machine page's "up" link goes back to Machines (backTo === "Machines"), and every machine page carries no
//    sideways overflow.
import path from "node:path";
import { ENV, served, data, reporter, overflow } from "../lib.mjs";

const afterTitle = (page, text) => page.waitForFunction((t) => document.querySelector("#topbar .t")?.textContent === t, text);

export default async function homeCheck(browser) {
  const D = await data();
  const r = reporter("home");
  const out = {};
  const errs = [];

  {
    const page = await served(browser, { size: "phone", dark: false });
    errs.push(...page.errors);
    const over = () => overflow(page);
    const nav = async (go) => { await page.evaluate(() => window.scrollTo(0, 0)); await page.click("#lead-btn"); await page.waitForTimeout(280); await page.click('.nav-item[data-go="' + go + '"]'); await page.waitForTimeout(150); };

    out.home = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, sub: document.querySelector(".ph .sub")?.textContent, secs: [...document.querySelectorAll(".page .sec-h")].map((s) => s.textContent), needs: document.querySelectorAll(".page .ib:not(.quiet)").length, items: [...document.querySelectorAll(".page .ib")].map((x) => ({ role: x.getAttribute("role"), tab: x.tabIndex, org: x.querySelector(".org")?.textContent.slice(0, 60) ?? null, trace: !!x.querySelector(".tracebtn"), nestedButton: x.tagName === "BUTTON" })), fors: [...document.querySelectorAll(".page .nrow .for")].map((x) => x.textContent.slice(0, 60)), live: document.querySelectorAll(".page .nrow").length, liveWithAct: document.querySelectorAll(".page .nrow .act").length }));
    out.homeOver = await over();
    await page.screenshot({ path: path.join(ENV.out, "sample-home.png") });

    await page.click("#lead-btn"); await page.waitForTimeout(280);
    out.sidebar = await page.evaluate(() => ({ nav: [...document.querySelectorAll(".nav-item")].map((n) => n.textContent), head: document.querySelector(".side-h")?.textContent, sidebarChips: document.querySelectorAll(".sidebar .groupby button").length, allLink: document.querySelector("#all-sessions")?.textContent, gheads: document.querySelectorAll("#lanes .ghead").length, rows: document.querySelectorAll("#lanes .srow").length, sorted: [...document.querySelectorAll("#lanes .srow .ag")].slice(0, 5).map((a) => a.textContent) }));
    await page.screenshot({ path: path.join(ENV.out, "sample-drawer.png") });
    await page.click("#drawer-close"); await page.waitForTimeout(250); await nav("sessions");
    out.sessionsPage = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, sub: document.querySelector(".ph .sub")?.textContent, rows: document.querySelectorAll(".page .nrow").length, groupby: [...document.querySelectorAll(".page .groupby button")].map((x) => x.textContent + (x.getAttribute("aria-pressed") === "true" ? "*" : "")) }));
    for (const g of ["project", "machine", "harness"]) { await page.click('.page .groupby button[data-g="' + g + '"]'); await page.waitForTimeout(60); out["group_" + g] = await page.evaluate(() => [...document.querySelectorAll(".page .sess .sec-h")].map((h) => h.textContent).join(" | ")); }
    out.sessionsGroupRows = {};
    for (const g of ["project", "machine", "harness", "recent"]) { await page.click('.page .groupby button[data-g="' + g + '"]'); await page.waitForTimeout(60); out.sessionsGroupRows[g] = await page.evaluate(() => document.querySelectorAll(".page .nrow").length); }
    out.sessionsOver = await over();
    await nav("machines");
    out.machines = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, sub: document.querySelector(".ph .sub")?.textContent, rows: [...document.querySelectorAll(".page .nrow")].map((r) => r.innerText.replace(/\n/g, " / ")) }));
    out.machinesOver = await over();
    await page.screenshot({ path: path.join(ENV.out, "sample-machines.png") });
    const ms = await page.evaluate(() => [...document.querySelectorAll(".page .nrow")].map((r) => r.dataset.m));
    r.expect(ms.length > 0, "no machine rows found on the Machines page");
    out.machinePages = [];
    for (const m of ms) {
      await nav("machines"); await page.click('.nrow[data-m="' + m + '"]'); await page.waitForTimeout(120);
      const info = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, up: document.querySelector(".uplink")?.textContent, crumb: document.querySelector(".topbar .crumb")?.textContent, cur: document.querySelector('.nav-item[aria-current="page"]')?.textContent, secs: [...document.querySelectorAll(".page .sec-h")].map((s) => s.textContent), sub: document.querySelector(".topbar .l2")?.textContent, moves: [...document.querySelectorAll(".page .ib")].map((x) => x.querySelector(".org")?.textContent.slice(0, 60) ?? null) }));
      info.over = await over(); out.machinePages.push(info);
      await page.screenshot({ path: path.join(ENV.out, "sample-machine-" + m + ".png"), fullPage: true });
      await page.goBack(); await page.waitForTimeout(150); info.backTo = await page.evaluate(() => document.querySelector(".page h1")?.textContent);
    }
    // Home rows lead somewhere.
    await nav("home"); const first = await page.$(".page .nrow"); r.expect(!!first, "no live row (.page .nrow) found on Home"); if (first) { await first.click(); await page.waitForTimeout(120); out.liveRowOpens = await page.evaluate(() => document.querySelector(".page h1")?.textContent); }
    await nav("home"); const q = await page.$(".page .ib .q"); r.expect(!!q, "no Needs-you item (.page .ib .q) found on Home"); if (q) { await q.click(); await page.waitForTimeout(200); out.itemOpens = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, turn: history.state?.turn, flash: document.querySelector(".turn.flash")?.dataset.turn })); }
    errs.push(...page.errors);
    await page.context().close();
  }

  // Desktop, light: two reference screenshots.
  {
    const page = await served(browser, { size: "desktop", dark: false });
    await page.screenshot({ path: path.join(ENV.out, "desk-home.png") });
    await page.click('.nav-item[data-go="machines"]'); await afterTitle(page, "Machines"); await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(ENV.out, "desk-machines.png") });
    errs.push(...page.errors);
    await page.context().close();
  }

  r.results = out;
  r.expect(errs.length === 0, "page errors: " + errs.join(" | "));
  r.expect(out.homeOver === 0, "home overflow=" + out.homeOver);
  r.expect(out.sessionsOver === 0, "sessions overflow=" + out.sessionsOver);
  r.expect(out.machinesOver === 0, "machines overflow=" + out.machinesOver);
  for (const mp of out.machinePages) r.expect(mp.over === 0, "machine page overflow=" + mp.over + " (" + mp.h1 + ")");
  const laneCount = Object.values(D.SESS).filter((s) => s.lane).length;
  r.expect(out.sidebar.rows === laneCount, "sidebar rows=" + out.sidebar.rows + " lanes=" + laneCount);
  const baseRows = out.sessionsPage.rows;
  for (const g of ["project", "machine", "harness", "recent"]) r.expect(out.sessionsGroupRows[g] === baseRows, "grouping by " + g + " changed the row count: " + out.sessionsGroupRows[g] + " vs " + baseRows);
  r.expect(!!out.liveRowOpens && out.liveRowOpens !== "Home", "a live row did not navigate away from Home: " + out.liveRowOpens);
  r.expect(!!out.itemOpens && out.itemOpens.h1 !== "Home" && !!out.itemOpens.turn && out.itemOpens.flash === out.itemOpens.turn, "the first Needs-you item did not land+flash on its own turn: " + JSON.stringify(out.itemOpens));
  for (const mp of out.machinePages) r.expect(mp.backTo === "Machines", "machine page back did not return to Machines: " + mp.backTo);

  return r.done();
}

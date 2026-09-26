// Ported from the mockup's full.js. The original drove test-real.html (real logs) on phone dark only, expanding
// every collapsible thing on every lane's session screen and every trace: child work, tool-call steps, and every
// "Show more" on a handoff card, then opened every trace reachable from a turn's Trace button and expanded every
// "more" on its hops. It never drove the sample mockup at all.
//
// Ported to drive the served sample fixture instead (the same measurements, on the fixture's data), phone dark only
// (the original never ran phone light or desktop for this pass). Session ids are unchanged from the sample, so the
// lane loop ('.srow[data-id]') needs no id mapping. Navigation is done the same way the original did — real clicks
// on '#lead-btn' / '.srow' / the trace button — but each click that changes screen now also waits for the served
// page's title (and, for a session, its transcript) rather than only the original's fixed waitForTimeout (kept
// alongside, per the porting brief).
//
// Assertions (derived from what these counts stand for in the mockup):
//  - no page errors.
//  - zero overflow screens (T.overflowScreens === 0): nothing pokes past the 390px screen on any lane or trace.
//  - every "Show more" on a handoff card fully un-clips its text (moreStillClipped === 0) and never navigates the
//    page away (navigatedByMore === 0) — it is a text reveal, not a link.
//  - every "more" opened on a trace hop fully un-clips its text (hopMoreStillClipped === 0).
//  - the walk actually visited screens and at least one trace (T.screens > 0, T.traces > 0), so a broken lane list
//    or a missing Trace button would fail loudly instead of reporting an all-zero pass.
import path from "node:path";
import { ENV, served, data, reporter, overflow } from "../lib.mjs";

const afterTitle = (page, text) => page.waitForFunction((t) => document.querySelector("#topbar .t")?.textContent === t, text);

export default async function full(browser) {
  const D = await data();
  const r = reporter("full");
  const page = await served(browser, { size: "phone", dark: true });
  const over = () => overflow(page);

  const openAll = () => page.evaluate(() => {
    const r = { steps: 0, withInput: 0, cutNoInput: 0, more: 0, moreFull: 0, moreStillClipped: 0, navigatedByMore: 0 };
    for (let k = 0; k < 3; k++) document.querySelectorAll('.cw-toggle[aria-expanded="false"], .tsum[aria-expanded="false"]').forEach((x) => x.click());
    document.querySelectorAll('.step > button[aria-expanded="false"]').forEach((x) => { x.click(); r.steps++;
      const box = x.parentElement, arg = x.querySelector('.sa')?.textContent ?? '';
      if (box.querySelector('.out .io')) r.withInput++; else if (arg.endsWith('…')) r.cutNoInput++; });
    const before = location.href;
    document.querySelectorAll('.hcard .more').forEach((m) => { if (m.hidden) return; r.more++; m.click();
      const br = m.parentElement.querySelector('.brief'); if (br.scrollHeight <= br.clientHeight + 1) r.moreFull++; else r.moreStillClipped++; });
    if (location.href !== before) r.navigatedByMore++;
    return r;
  });

  const T = { screens: 0, overflowScreens: 0, steps: 0, withInput: 0, cutNoInput: 0, more: 0, moreFull: 0, moreStillClipped: 0, navigatedByMore: 0, traces: 0, hopMore: 0, hopMoreStillClipped: 0 };
  const add = (rr) => { for (const k in rr) T[k] += rr[k]; };
  const top = async () => { await page.evaluate(() => window.scrollTo(0, 0)); await page.mouse.wheel(0, -50); await page.waitForTimeout(350); };
  const openDrawer = async () => { await page.click("#lead-btn"); await page.waitForTimeout(280); };
  const openLane = async (id) => {
    await top(); await openDrawer(); await page.click('.srow[data-id="' + id + '"]');
    await afterTitle(page, D.SESS[id].name); await page.waitForFunction(() => !!document.querySelector("#page section[aria-label='Transcript']"));
    await page.waitForTimeout(150);
  };

  await top(); await openDrawer();
  const lanes = await page.evaluate(() => [...document.querySelectorAll(".srow")].map((row) => row.dataset.id));
  await page.click("#drawer-close"); await page.waitForTimeout(250);

  for (const id of lanes) {
    await openLane(id);
    add(await openAll()); await page.waitForTimeout(60); T.screens++; if (await over()) T.overflowScreens++;
    const tt = await page.evaluate(() => [...document.querySelectorAll(".turn-end .tracebtn")].map((b) => b.closest(".turn").dataset.turn));
    for (const t of tt) {
      await openLane(id);
      await page.click('.turn[data-turn="' + t + '"] > .turn-end .tracebtn');
      await afterTitle(page, "Trace"); await page.waitForTimeout(150); T.traces++;
      const rr = await page.evaluate(() => { let n = 0, c = 0; document.querySelectorAll(".hop .more:not([hidden])").forEach((m) => { n++; m.click(); const br = m.parentElement.querySelector(".brief"); if (br.scrollHeight > br.clientHeight + 1) c++; }); return [n, c]; });
      T.hopMore += rr[0]; T.hopMoreStillClipped += rr[1]; await page.waitForTimeout(60); T.screens++; if (await over()) T.overflowScreens++;
    }
  }

  // One screenshot of an expanded Bash step and an expanded relay, for the record.
  await openLane(lanes[0]);
  await page.evaluate(() => { document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click()); });
  const st = page.locator('.step > button:has(.sa:text-matches("…$"))').first();
  if (await st.count()) { await st.click(); await st.scrollIntoViewIfNeeded(); await page.evaluate(() => window.scrollBy(0, -120)); await page.waitForTimeout(400); await page.screenshot({ path: path.join(ENV.out, "full-step.png") }); }
  const mo = page.locator(".hcard .more:visible").first();
  if (await mo.count()) { await mo.click(); await mo.scrollIntoViewIfNeeded(); await page.evaluate(() => window.scrollBy(0, -300)); await page.waitForTimeout(400); await page.screenshot({ path: path.join(ENV.out, "full-relay.png") }); }

  r.results.tally = T;
  r.results.errors = page.errors;
  r.expect(page.errors.length === 0, "page errors: " + page.errors.join(" | "));
  r.expect(T.overflowScreens === 0, "overflowScreens=" + T.overflowScreens);
  r.expect(T.moreStillClipped === 0, "moreStillClipped=" + T.moreStillClipped);
  r.expect(T.navigatedByMore === 0, "navigatedByMore=" + T.navigatedByMore);
  r.expect(T.hopMoreStillClipped === 0, "hopMoreStillClipped=" + T.hopMoreStillClipped);
  r.expect(T.screens > 0, "no screens visited");
  r.expect(T.traces > 0, "no traces opened");

  await page.context().close();
  return r.done();
}

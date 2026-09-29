// Switching sessions paints at once (#29 item 2a), on the primary fixture, phone and desktop. The sidebar's sessions are
// opened by clicking their rows, with /api/tx held back by a route where a check needs the response to be late:
//   - within one frame of a click (at most 50 ms), the row is current and the top bar names the new session; until 150 ms the old
//     page stays, dimmed and inert with aria-busy, and no skeleton is drawn;
//   - with /api/tx delayed by a second, the skeleton shows (six turn-shaped placeholders, aria-hidden, no sideways overflow),
//     then the transcript replaces it with the top bar where it was, aria-busy off and focus on the session's title;
//   - a fast double switch (A, then B while A's response is late) ends on B: A's response is dropped and never flashes;
//   - a session left a moment ago draws at once from the cache, without waiting for /api/tx and with none requested for it;
//   - the cache is bounded: after seven sessions the first is gone (it waits for the network again), a recent one is not.
// Skeleton screenshots at 390 and 1280, light and dark, go to out/switch-skeleton-*.png.
import path from "node:path";
import { ENV, served, data, reporter, overflow } from "../lib.mjs";

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const plain = (text) => String(text ?? "").replace(/ /g, "").replace(/\s+/g, " ").trim();
const HELD = 1000;

// The page with /api/tx held back per session (`delays`: sid, or "*" for any) and every request noted in `seen`.
async function open(browser, opts) {
  const page = await served(browser, opts);
  page.delays = new Map();
  page.seen = [];
  await page.route("**/api/tx*", async (route) => {
    const sid = new URL(route.request().url()).searchParams.get("sid");
    const ms = page.delays.get(sid) ?? page.delays.get("*") ?? 0;
    page.seen.push(sid);
    if (ms) await sleep(ms);
    try { await route.continue(); } catch {} // the page may have cancelled the request meanwhile
  });
  return page;
}

// The sidebar's session rows, top to bottom (the drawer holds them on a phone, hidden but in the page).
const rowsOf = (page) => page.evaluate(() => [...document.querySelectorAll("#lanes .srow[data-id]")].map((x) => x.dataset.id));

// Clicks a row and reads the page in the same task, so what it reports is what the click drew synchronously.
const click = (page, id) => page.evaluate((id) => {
  const row = [...document.querySelectorAll("#lanes .srow[data-id]")].find((x) => x.dataset.id === id), t0 = performance.now();
  row.click();
  const ms = performance.now() - t0, main = document.querySelector("#page");
  return {
    ms,
    current: document.querySelector('#lanes .srow[aria-current="page"]')?.dataset.id ?? null,
    title: document.querySelector("#topbar .t")?.textContent ?? null,
    busy: main.getAttribute("aria-busy") === "true",
    inert: main.inert,
    dimmed: main.classList.contains("loading"),
    skeleton: document.querySelectorAll(".skeleton").length,
    transcript: !!main.querySelector("section[aria-label='Transcript']"),
    heading: main.querySelector(".ph h1")?.textContent ?? null,
  };
}, id);

const landed = (page, id, name) => page.waitForFunction(([id, name]) => {
  const main = document.querySelector("#page"), plain = (t) => String(t ?? "").replace(/\u2009/g, "").replace(/\s+/g, " ").trim();
  return !document.querySelector(".skeleton") && main.getAttribute("aria-busy") !== "true" && !!main.querySelector("section[aria-label='Transcript']")
    && plain(main.querySelector(".ph h1")?.textContent) === name && location.pathname.endsWith("/" + encodeURIComponent(id));
}, [id, name], { timeout: 10_000 });

const barBox = (page) => page.evaluate(() => {
  const bar = document.querySelector("#topbar").getBoundingClientRect(), t = document.querySelector("#topbar .t").getBoundingClientRect();
  return { barH: Math.round(bar.height * 10) / 10, top: Math.round(t.top * 10) / 10, left: Math.round(t.left * 10) / 10, w: Math.round(t.width * 10) / 10 };
});

export default async function switchCheck(browser) {
  const D = await data();
  const r = reporter("switch");
  const R = r.results;
  const nameOf = (id) => plain(D.SESS[id]?.name);

  // ---- One screen: the first-frame paint, the skeleton, the landing --------------------------------------------------
  async function screen(size, dark, full) {
    const tag = size + (dark ? "-dark" : "-light"), out = (R[tag] = {});
    const page = await open(browser, { size, dark });
    const rows = await rowsOf(page);
    r.expect(rows.length >= 7, tag + ": the sidebar needs at least 7 session rows, has " + rows.length);
    if (rows.length < 7) { await page.context().close(); return; }
    const [a, b] = rows;
    await click(page, a);
    await landed(page, a, nameOf(a));

    // The response is held for a second: what the click draws at once, at 30 ms, and once the skeleton is up.
    page.delays.set(b, HELD);
    const first = await click(page, b);
    await sleep(30);
    const early = await page.evaluate(() => ({ skeleton: document.querySelectorAll(".skeleton").length, busy: document.querySelector("#page").getAttribute("aria-busy") === "true" }));
    out.first = { ...first, ms: Math.round(first.ms * 10) / 10 }; out.early = early;
    r.expect(first.current === b, tag + ": the clicked row is not current right after the click: " + first.current);
    r.expect(plain(first.title) === nameOf(b), tag + ": the top bar does not name the new session right after the click: " + first.title);
    r.expect(first.ms <= 50, tag + ": the click took " + first.ms + " ms to draw the row and the top bar (limit 50)");
    r.expect(first.busy && first.inert && first.dimmed && first.transcript && first.skeleton === 0, tag + ": before 150 ms the old page should stay, dimmed, inert and aria-busy, with no skeleton: " + JSON.stringify(first));
    r.expect(early.busy && early.skeleton === 0, tag + ": a skeleton was drawn before 150 ms: " + JSON.stringify(early));
    await page.waitForSelector(".skeleton", { state: "attached", timeout: 3000 });
    const bar1 = await barBox(page);
    const skeleton = await page.evaluate(() => {
      const boxes = [...document.querySelectorAll(".skeleton")], main = document.querySelector("#page");
      return { count: boxes.length, hidden: boxes.every((x) => x.getAttribute("aria-hidden") === "true"), turns: document.querySelectorAll(".skeleton .sk-turn").length, busy: main.getAttribute("aria-busy") === "true", oldTranscript: !!main.querySelector("section[aria-label='Transcript']") };
    });
    out.skeleton = skeleton;
    r.expect(skeleton.count === 1 && skeleton.hidden && skeleton.turns === 6 && skeleton.busy && !skeleton.oldTranscript, tag + ": the skeleton is not one aria-hidden block of six turns replacing the old transcript: " + JSON.stringify(skeleton));
    out.overflowSkeleton = await overflow(page);
    r.expect(out.overflowSkeleton === 0, tag + ": the skeleton overflows sideways: " + out.overflowSkeleton);
    await page.screenshot({ path: path.join(ENV.out, "switch-skeleton-" + tag + ".png") });
    await landed(page, b, nameOf(b));
    const bar2 = await barBox(page), after = await page.evaluate(() => ({ active: document.activeElement?.tagName ?? null, heading: document.activeElement?.textContent ?? null, busy: document.querySelector("#page").hasAttribute("aria-busy"), inert: document.querySelector("#page").inert, skeleton: document.querySelectorAll(".skeleton").length }));
    out.bar = [bar1, bar2]; out.after = after;
    r.expect(JSON.stringify(bar1) === JSON.stringify(bar2), tag + ": the top bar moved when the transcript landed: " + JSON.stringify([bar1, bar2]));
    r.expect(after.active === "H1" && plain(after.heading) === nameOf(b), tag + ": focus did not move to the session's title once the transcript landed: " + JSON.stringify(after));
    r.expect(!after.busy && !after.inert && after.skeleton === 0, tag + ": the page is still busy, inert or showing a skeleton after landing: " + JSON.stringify(after));
    out.overflowLanded = await overflow(page);
    r.expect(out.overflowLanded === 0, tag + ": the page overflows sideways after landing: " + out.overflowLanded);
    if (!full) { r.expect(page.errors.length === 0, tag + ": page errors: " + page.errors.join(" | ")); await page.context().close(); return; }

    // ---- A fast double switch: A's response is late and must be dropped ------------------------------------------------
    const [c, d] = [rows[2], rows[3]];
    page.delays.clear(); page.delays.set(c, 1500);
    await click(page, c);
    await sleep(100);
    await click(page, d);
    await landed(page, d, nameOf(d));
    await sleep(1800); // past the moment C's response would have arrived
    out.double = await page.evaluate(() => ({ path: location.pathname, title: document.querySelector("#topbar .t")?.textContent ?? null, heading: document.querySelector("#page .ph h1")?.textContent ?? null, skeleton: document.querySelectorAll(".skeleton").length, current: document.querySelector('#lanes .srow[aria-current="page"]')?.dataset.id ?? null }));
    r.expect(out.double.path.endsWith("/" + encodeURIComponent(d)) && plain(out.double.title) === nameOf(d) && plain(out.double.heading) === nameOf(d) && out.double.current === d && out.double.skeleton === 0, tag + ": a fast double switch did not end on the second session: " + JSON.stringify(out.double));

    // ---- Back to a session left a moment ago (a): from the cache, with the network held ------------------------------------
    page.delays.clear(); page.delays.set("*", 1500); page.seen.length = 0;
    const hit = await click(page, a);
    out.hit = { ...hit, ms: Math.round(hit.ms * 10) / 10 };
    r.expect(!hit.busy && hit.transcript && !hit.skeleton && plain(hit.heading) === nameOf(a) && plain(hit.title) === nameOf(a) && hit.current === a, tag + ": a session left a moment ago did not draw at once from the cache: " + JSON.stringify(hit));
    r.expect(hit.ms <= 50, tag + ": the cached session took " + hit.ms + " ms to draw (limit 50)");
    await sleep(400);
    r.expect(!page.seen.includes(a), tag + ": the cached session asked /api/tx for its own transcript although nothing had changed");
    await sleep(1600); // child work held for 1.5 s arrives and the page is drawn again; nothing may break
    out.afterHit = await page.evaluate(() => ({ heading: document.querySelector("#page .ph h1")?.textContent ?? null, transcript: !!document.querySelector("#page section[aria-label='Transcript']"), skeleton: document.querySelectorAll(".skeleton").length }));
    r.expect(plain(out.afterHit.heading) === nameOf(a) && out.afterHit.transcript && !out.afterHit.skeleton, tag + ": the cached session broke when its child work arrived: " + JSON.stringify(out.afterHit));
    r.expect(page.errors.length === 0, tag + ": page errors: " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- The cache is bounded: seven sessions, then the first is gone and a recent one is not -------------------------------
  async function bounded(size) {
    const tag = "bounded-" + size, out = (R[tag] = {});
    const page = await open(browser, { size });
    const rows = (await rowsOf(page)).slice(0, 7);
    r.expect(rows.length === 7, tag + ": the sidebar needs 7 session rows, has " + rows.length);
    if (rows.length < 7) { await page.context().close(); return; }
    for (const id of rows) { await click(page, id); await landed(page, id, nameOf(id)); }
    page.delays.set("*", 1500);
    // The five kept are rows 2 to 6 (the seventh is open): the first is out, and so, once the first is opened, is the second.
    const oldest = await click(page, rows[0]);
    await landed(page, rows[0], nameOf(rows[0]));
    const recent = await click(page, rows[5]);
    await landed(page, rows[5], nameOf(rows[5]));
    const second = await click(page, rows[1]);
    out.busy = { oldest: oldest.busy, recent: recent.busy, second: second.busy };
    r.expect(oldest.busy, tag + ": the first of seven sessions was still in the cache");
    r.expect(!recent.busy && plain(recent.heading) === nameOf(rows[5]), tag + ": a recently left session was not drawn from the cache: " + JSON.stringify(recent));
    r.expect(second.busy, tag + ": the cache kept more than five sessions");
    r.expect(page.errors.length === 0, tag + ": page errors: " + page.errors.join(" | "));
    await page.context().close();
  }

  await screen("phone", false, true);
  await screen("desktop", false, true);
  await screen("phone", true, false);
  await screen("desktop", true, false);
  await bounded("desktop");
  return r.done();
}

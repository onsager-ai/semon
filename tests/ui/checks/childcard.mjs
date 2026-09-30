// A subagent's spawn card keeps its actions row where it is when the "Activity" disclosure opens: Open and the disclosure stay
// side by side at the same top, left and height, the chevron turns in place, and the opened activity sits below the whole row (not
// between Open and the disclosure, and without a gap). Closing it puts every box back. Run at 390x844 and 1280x800, light and dark,
// on the first spawn card that has activity to show; screenshots of the card collapsed and opened go to out/childcard/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, goto, reporter } from "../lib.mjs";

const OUT = path.join(ENV.out, "childcard");
fs.mkdirSync(OUT, { recursive: true });
const near = (a, b) => Math.abs(a - b) <= 1;
function paintFixture(D) {
  const candidates = [];
  for (const h of D.H) {
    if (h.kind !== "spawn" || !D.SESS[h.from] || !D.SESS[h.to]) continue;
    const childTurn = D.turns.find((t) => t.sid === h.to && t.start === h.id), parentTurns = D.turns.filter((t) => t.sid === h.from);
    const parentTurn = parentTurns.find((t) => t.sent.includes(h.id)), parentRank = parentTurns.indexOf(parentTurn);
    if (!childTurn || !parentTurn || parentRank < 1) continue;
    const activity = (D.TX[h.to] ?? []).filter((e) => e.turn === childTurn.id && !(e.k === "h" && e.id === h.id));
    const handoffAt = (D.TX[h.from] ?? []).findIndex((e) => e.k === "h" && e.id === h.id);
    if (activity.length && handoffAt >= 0) candidates.push({ parent: h.from, child: h.to, spawn: h.id, childTurn: childTurn.id, parentTurn: parentTurn.id, rank: handoffAt });
  }
  return candidates.sort((a, b) => b.rank - a.rank)[0] ?? null;
}
const childTurnRoute = (fixture) => (url) => url.pathname === "/api/tx" && url.searchParams.get("sid") === fixture.child && url.searchParams.get("turn") === fixture.childTurn;
async function drawnWithoutChildWork(page, D, fixture, turn = null, timeout = 3000) {
  return page.waitForFunction((x) => {
    const card = [...document.querySelectorAll("#page .turns .hcard.child-card")].find((c) => c.dataset.h === x.spawn && !c.closest(".cw-body"));
    return document.querySelector("#topbar .t")?.textContent === x.title
      && !!document.querySelector("#page section[aria-label='Transcript'] .turn")
      && (!x.turn || !!document.querySelector('.turn[data-turn="' + CSS.escape(x.turn) + '"]'))
      && !!card && !card.querySelector(".child-work");
  }, { title: D.SESS[fixture.parent].name, spawn: fixture.spawn, turn }, { timeout }).then(() => true, () => false);
}
async function frames(page) { await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))); }

// The disclosure reads "Activity", with the count of steps quietly after it (the count "Show all N" uses), and names whose activity it is
// only for assistive technology.
// The boxes the row is made of, rounded to a tenth of a pixel.
const measure = (page) => page.evaluate(() => {
  const c = [...document.querySelectorAll("#page .turns .hcard.child-card")].find((x) => !x.closest(".cw-body") && x.querySelector(".child-actions .cw-toggle"));
  if (!c) return null;
  const r1 = (v) => Math.round(v * 10) / 10, box = (n) => { const b = n.getBoundingClientRect(); return { top: r1(b.top), left: r1(b.left), width: r1(b.width), height: r1(b.height), bottom: r1(b.bottom) }; };
  const open = c.querySelector(".child-actions > button:not(.cw-toggle)"), toggle = c.querySelector(".child-actions .cw-toggle"), body = c.querySelector(".child-actions .cw-body");
  const o = box(open), t = box(toggle), row = { top: Math.min(o.top, t.top), bottom: Math.max(o.bottom, t.bottom) };
  return { open: o, toggle: t, row: { ...row, height: r1(row.bottom - row.top) }, card: box(c), expanded: toggle.getAttribute("aria-expanded"), bodyShown: !!body && !body.hidden && body.getClientRects().length > 0, body: body && !body.hidden ? box(body) : null, label: toggle.textContent.replace(/[\u2009\u00a0]/g, " ").replace(/\s+/g, " ").trim(), aria: toggle.getAttribute("aria-label"), childName: (c.querySelector(".child-head .ln")?.textContent ?? "").replace(/[\u2009\u00a0]/g, " "), showAll: c.querySelector(".child-actions .cw-body .show-all")?.textContent ?? null, chevron: getComputedStyle(toggle.querySelector(".chev")).transform };
});

export default async function childcard(browser) {
  const D = await data();
  const r = reporter("childcard");
  const lanes = Object.keys(D.SESS);
  r.results.runs = [];
  for (const size of ["phone", "desktop"]) {
    for (const dark of [false, true]) {
      const name = size + (dark ? "-dark" : "-light");
      const page = await served(browser, { size, dark });
      let m0 = null;
      for (const id of lanes) {
        await goto(page, { v: "session", id }, D);
        const hasActivity = D.H.some((h) => h.kind === "spawn" && h.from === id && (D.TX[h.to] ?? []).some((e) => e.k !== "h" || e.id !== h.id));
        if (hasActivity) await page.waitForFunction(() => [...document.querySelectorAll("#page .turns .hcard.child-card")].some((c) => !c.closest(".cw-body") && c.querySelector(".child-actions .cw-toggle")), null, { timeout: 5000 }).catch(() => {});
        m0 = await measure(page);
        if (m0) break;
      }
      r.expect(!!m0, name + ": the fixture needs a spawn card with activity to show");
      if (!m0) { await page.context().close(); continue; }
      const card = page.locator("#page .turns .hcard.child-card:has(.child-actions .cw-toggle)").first();
      const toggle = card.locator(".child-actions .cw-toggle");
      await card.scrollIntoViewIfNeeded(); await page.waitForTimeout(80);
      const m1 = await measure(page);
      await card.screenshot({ path: path.join(OUT, name + "-collapsed.png") });
      await toggle.click(); await page.waitForTimeout(250);
      const m2 = await measure(page);
      await card.screenshot({ path: path.join(OUT, name + "-open.png") });
      await toggle.click(); await page.waitForTimeout(250);
      const m3 = await measure(page);
      r.results.runs.push({ name, collapsed: m1, open: m2, closed: m3 });
      const at = (m) => name + ": " + JSON.stringify({ open: m.open, toggle: m.toggle, row: m.row });
      r.expect(m1.expanded === "false" && !m1.bodyShown, name + ": collapsed to begin with");
      r.expect(m2.expanded === "true" && m2.bodyShown, name + ": the disclosure opens its activity: " + JSON.stringify({ expanded: m2.expanded, shown: m2.bodyShown }));
      r.expect(/^Activity · \d+$/.test(m1.label) && !/\bdid\b/.test(m1.label), name + ": the disclosure reads \"Activity · N\": " + JSON.stringify(m1.label));
      r.expect(m1.aria === "Activity of " + m1.childName, name + ": its accessible name says whose activity it is: " + JSON.stringify([m1.aria, m1.childName]));
      if (m2.showAll) r.expect(m2.showAll === "Show all " + m1.label.replace(/^Activity · /, ""), name + ": the count is the one Show all uses: " + JSON.stringify([m1.label, m2.showAll]));
      // Open and the disclosure keep their place: top, left and height (within a pixel) and the row keeps its height.
      for (const k of ["open", "toggle"]) {
        r.expect(near(m1[k].top, m2[k].top) && near(m1[k].left, m2[k].left) && near(m1[k].height, m2[k].height), name + ": " + k + " stays put when the disclosure opens: before " + JSON.stringify(m1[k]) + ", after " + JSON.stringify(m2[k]));
      }
      r.expect(near(m1.row.height, m2.row.height), name + ": the row keeps its height: " + m1.row.height + " then " + m2.row.height);
      r.expect(near(m2.open.top, m2.toggle.top) || near(m2.open.top + m2.open.height / 2, m2.toggle.top + m2.toggle.height / 2), name + ": Open and the disclosure share a row when open: " + at(m2));
      r.expect(m2.toggle.left >= m2.open.left + m2.open.width - 1, name + ": the disclosure sits beside Open, to its right: " + at(m2));
      // The activity sits below the whole row, with no gap beyond the card's own spacing.
      r.expect(!!m2.body && m2.body.top >= m2.row.bottom - 1, name + ": the activity starts below the row: body " + JSON.stringify(m2.body) + ", row " + JSON.stringify(m2.row));
      r.expect(!!m2.body && m2.body.top - m2.row.bottom <= 10, name + ": no gap between the row and its activity: " + (m2.body ? m2.body.top - m2.row.bottom : "none"));
      r.expect(near(m1.card.top, m2.card.top), name + ": the card's top does not move: " + m1.card.top + " then " + m2.card.top);
      // The chevron turns in place (a rotation, not a move), and turns back.
      r.expect(m1.chevron !== m2.chevron && m3.chevron === m1.chevron, name + ": the chevron turns and turns back: " + [m1.chevron, m2.chevron, m3.chevron].join(" | "));
      // Closing puts every box back.
      r.expect(m3.expanded === "false" && !m3.bodyShown, name + ": closed again");
      for (const k of ["open", "toggle"]) r.expect(near(m1[k].top, m3[k].top) && near(m1[k].left, m3[k].left) && near(m1[k].height, m3[k].height), name + ": " + k + " is back where it was after closing: " + JSON.stringify(m1[k]) + " then " + JSON.stringify(m3[k]));
      r.expect(near(m1.card.height, m3.card.height), name + ": the card is its old height after closing: " + m1.card.height + " then " + m3.card.height);
      r.expect(page.errors.length === 0, name + ": page errors: " + page.errors.join(" | "));
      await page.context().close();
    }
  }

  const fixture = paintFixture(D);
  r.expect(!!fixture, "paint first: the fixture needs a spawned child with activity in a later parent turn");
  if (fixture) {
    for (const [size, suffix] of [["phone", "390-light"], ["desktop", "1280-light"]]) {
      const page = await served(browser, { size, dark: false });
      let release;
      const gate = new Promise((resolve) => { release = resolve; });
      let requested;
      const requestSeen = new Promise((resolve) => { requested = resolve; });
      await page.route(childTurnRoute(fixture), async (route) => { requested(); await gate; await route.continue(); });
      const started = Date.now();
      await goto(page, { v: "session", id: fixture.parent }, D);
      const remaining = 3000 - (Date.now() - started), drawn = remaining > 0 && await drawnWithoutChildWork(page, D, fixture, null, remaining);
      r.expect(drawn, suffix + ": the parent title and turns draw with the child card unfilled within 3 s");
      const held = await Promise.race([requestSeen.then(() => true), page.waitForTimeout(3000).then(() => false)]);
      r.expect(held, suffix + ": the child turn request was held");
      const card = page.locator('#page .turns .hcard.child-card[data-h="' + fixture.spawn + '"]');
      if (drawn) await card.screenshot({ path: path.join(OUT, "paintfirst-" + suffix + ".png") });
      if (size === "phone" && held) {
        await page.mouse.wheel(0, -10000); // End the existing open-at-end pin before measuring the reader's scroll.
        const place = await page.evaluate((id) => {
          const c = [...document.querySelectorAll("#page .turns .hcard.child-card")].find((x) => x.dataset.h === id);
          window.scrollTo(0, 0);
          return { top: c?.getBoundingClientRect().top ?? -1, height: innerHeight, y: window.scrollY };
        }, fixture.spawn);
        r.expect(place.top > place.height, "390-light: the held child card is below the viewport for the scroll check: " + JSON.stringify(place));
        release();
        const filled = await page.waitForFunction((id) => [...document.querySelectorAll("#page .turns .hcard.child-card")].find((c) => c.dataset.h === id)?.querySelector(".child-work"), fixture.spawn, { timeout: 5000 }).then(() => true, () => false);
        await frames(page);
        const after = await page.evaluate(() => window.scrollY);
        r.expect(filled, "390-light: the child card's activity appears after release");
        r.expect(Math.abs(after - place.y) <= 2, "390-light: window.scrollY stays put when activity below the viewport arrives: " + place.y + " then " + after);
      } else release();
      await page.context().close();
    }

    const deep = await served(browser, { size: "desktop", dark: true });
    let releaseDeep;
    const deepGate = new Promise((resolve) => { releaseDeep = resolve; });
    let deepRequested;
    const deepRequestSeen = new Promise((resolve) => { deepRequested = resolve; });
    await deep.route(childTurnRoute(fixture), async (route) => { deepRequested(); await deepGate; await route.continue(); });
    const deepStarted = Date.now();
    await goto(deep, { v: "session", id: fixture.parent, turn: fixture.parentTurn }, D);
    const deepRemaining = 3000 - (Date.now() - deepStarted), deepDrawn = deepRemaining > 0 && await drawnWithoutChildWork(deep, D, fixture, fixture.parentTurn, deepRemaining);
    r.expect(deepDrawn, "1280-dark: the deep-linked parent turn draws with its child card unfilled within 3 s");
    const deepHeld = await Promise.race([deepRequestSeen.then(() => true), deep.waitForTimeout(3000).then(() => false)]);
    r.expect(deepHeld, "1280-dark: the deep-linked child turn request was held");
    const anchor = deep.locator('.turn[data-turn="' + fixture.parentTurn + '"]');
    const topBefore = await anchor.evaluate((n) => n.getBoundingClientRect().top);
    releaseDeep();
    const deepFilled = await deep.waitForFunction((id) => [...document.querySelectorAll("#page .turns .hcard.child-card")].find((c) => c.dataset.h === id)?.querySelector(".child-work"), fixture.spawn, { timeout: 5000 }).then(() => true, () => false);
    await frames(deep);
    const tops = await deep.evaluate((id) => ({ turn: document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]')?.getBoundingClientRect().top ?? null, bar: document.querySelector("#topbar").getBoundingClientRect().bottom }), fixture.parentTurn);
    r.expect(deepFilled, "1280-dark: the child card's activity appears after release");
    r.expect(tops.turn != null && Math.abs(tops.turn - topBefore) <= 8 && Math.abs(tops.turn - tops.bar - 8) <= 8, "1280-dark: the deep-linked parent turn stays at the top after its child fills: " + JSON.stringify({ before: topBefore, ...tops }));
    if (deepFilled) {
      const card = deep.locator('#page .turns .hcard.child-card[data-h="' + fixture.spawn + '"]');
      await card.locator(".cw-toggle").click();
      await deep.waitForFunction((id) => [...document.querySelectorAll("#page .turns .hcard.child-card")].find((c) => c.dataset.h === id)?.querySelector(".cw-body:not([hidden])"), fixture.spawn, { timeout: 3000 });
      await card.screenshot({ path: path.join(OUT, "paintfirst-1280-dark.png") });
    }
    await deep.context().close();

    const failedPage = await served(browser, { size: "phone", dark: false });
    let failDone;
    const failed = new Promise((resolve) => { failDone = resolve; });
    let failRequested;
    const failRequestSeen = new Promise((resolve) => { failRequested = resolve; });
    await failedPage.route(childTurnRoute(fixture), async (route) => { failRequested(); await route.fulfill({ status: 500, contentType: "text/plain", body: "no" }); failDone(); });
    const failedStarted = Date.now();
    await goto(failedPage, { v: "session", id: fixture.parent }, D);
    const failureRemaining = 3000 - (Date.now() - failedStarted), failureDrawn = failureRemaining > 0 && await drawnWithoutChildWork(failedPage, D, fixture, null, failureRemaining);
    r.expect(failureDrawn, "child failure: the parent title and turns draw while its child request fails");
    const failureRequested = await Promise.race([failRequestSeen.then(() => true), failedPage.waitForTimeout(3000).then(() => false)]);
    r.expect(failureRequested, "child failure: the child's turn request was made");
    const failureReturned = await Promise.race([failed.then(() => true), failedPage.waitForTimeout(3000).then(() => false)]);
    await frames(failedPage);
    const stillEmpty = await failedPage.evaluate((id) => { const c = [...document.querySelectorAll("#page .turns .hcard.child-card")].find((x) => x.dataset.h === id); return !!c && !c.querySelector(".child-work"); }, fixture.spawn);
    r.expect(failureReturned && stillEmpty, "child failure: a 500 leaves the child card without inline work");
    await failedPage.context().close();
  }
  return r.done();
}

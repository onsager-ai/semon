// Ported from the mockup's check-real.js: a phone-dark walk of Home, Sessions and Machines, then every lane's
// session screen (collapsed and fully opened — every collapsed group and every collapsed step), and every trace
// reachable from a turn's Trace button (rail alignment included), tallying screens, overflow, traces, hops and rail
// alignment. The original only ever drove test-real.html (real logs); it never had a sample pass at all.
//
// Ported to drive the served sample fixture instead (the same measurements, on the fixture's data). Every lookup
// here is by session id or by a live DOM query, so there is no id mapping to do. The sidebar is now a recent-session
// tree, so this exhaustive fixture walk routes directly to every served session; Home covers sidebar navigation.
//
// Assertions:
//  - no page errors.
//  - zero overflow screens.
//  - every lane has at least one transcript item once opened (emptyTranscripts === 0) — a lane with nothing to show
//    would mean the walk found a session the fixture didn't actually populate.
//  - the rail (the vertical line connecting a trace's hops) has no horizontal offset and no vertical gaps
//    (railOff === 0, railGaps === 0).
//  - the walk actually visited lanes, turns and traces (lanes > 0, turns > 0, traces > 0), so a broken lane list or
//    a missing Trace button fails loudly instead of an all-zero pass.
import path from 'node:path';
import { ENV, served, data, goto, reporter, overflow } from '../lib.mjs';

// The title is in the bar as soon as a session is clicked; the page is ready once it is no longer aria-busy.
const afterTitle = (page, text) =>
  page.waitForFunction(
    (t) =>
      document.querySelector('#topbar .t')?.textContent === t &&
      !document.querySelector('#page').hasAttribute('aria-busy'),
    text,
  );

export default async function checkReal(browser) {
  const D = await data();
  const r = reporter('check-real');
  const page = await served(browser, { size: 'phone', dark: true });
  const over = () => overflow(page);
  const rail = () =>
    page.evaluate(() => {
      let off = 0,
        gaps = 0;
      const hops = [...document.querySelectorAll('.hop')];
      hops.forEach((h, i) => {
        const hb = h.getBoundingClientRect(),
          nb = h.querySelector('.node').getBoundingClientRect(),
          cs = getComputedStyle(h, '::before');
        if (Math.abs(hb.left + parseFloat(cs.left) + 1 - (nb.left + nb.width / 2)) > 0.5) off++;
        if (i && Math.abs(hops[i - 1].getBoundingClientRect().bottom - hb.top) > 0.5) gaps++;
      });
      return [hops.length, off, gaps];
    });
  const tally = {
    screens: 0,
    overflowScreens: 0,
    traces: 0,
    hops: 0,
    railOff: 0,
    railGaps: 0,
    lanes: 0,
    turns: 0,
    emptyTranscripts: 0,
    convItems: 0,
    moreOpened: 0,
  };
  const nav = async (go) => {
    await page.click('#lead-btn');
    await page.waitForTimeout(280);
    await page.click('.nav-item[data-go="' + go + '"]');
    const titles = { home: 'Home', sessions: 'Sessions', machines: 'Machines' };
    if (titles[go]) await afterTitle(page, titles[go]);
    await page.waitForTimeout(150);
  };

  for (const go of ['home', 'sessions', 'machines']) {
    await nav(go);
    tally.screens++;
    if (await over()) tally.overflowScreens++;
  }
  await page.screenshot({ path: path.join(ENV.out, 'check-real-home.png') });
  const lanes = Object.keys(D.SESS);
  const openLane = async (id) => {
    await goto(page, { v: 'session', id }, D);
    await page.waitForTimeout(100);
  };

  for (const id of lanes) {
    await openLane(id);
    tally.lanes++;
    tally.screens++;
    if (await over()) tally.overflowScreens++;
    await page.evaluate(() => {
      document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((b) => b.click());
      document.querySelectorAll('.step > button[aria-expanded="false"]').forEach((b) => b.click());
    });
    await page.waitForTimeout(80);
    tally.screens++;
    if (await over()) tally.overflowScreens++;
    const conv = await page.evaluate(
      () =>
        document.querySelectorAll(
          '.tx .msg, .tx .step, .tx .event, .tx .child-card, .tx .bubble, .tx .tgroup',
        ).length,
    );
    tally.convItems += conv;
    if (!conv) tally.emptyTranscripts++;
    tally.turns += await page.evaluate(() => document.querySelectorAll('.turns > .turn').length);
    tally.moreOpened += await page.evaluate(() => {
      let n = 0;
      document.querySelectorAll('.event .ev-more:not([hidden])').forEach((b) => {
        b.click();
        n++;
      });
      return n;
    });
    await page.waitForTimeout(60);
    tally.screens++;
    if (await over()) tally.overflowScreens++;
    const tt = await page.evaluate(() =>
      [...document.querySelectorAll('.turn-end .link')].map((b) => b.closest('.turn').dataset.turn),
    );
    for (const t of tt) {
      await openLane(id);
      await page.click('.turn[data-turn="' + t + '"] > .turn-end .link');
      await afterTitle(page, 'Trace');
      await page.waitForTimeout(100);
      tally.traces++;
      tally.screens++;
      if (await over()) tally.overflowScreens++;
      await page.evaluate(() =>
        document.querySelectorAll('.hop .more:not([hidden])').forEach((b) => b.click()),
      );
      await page.waitForTimeout(60);
      tally.screens++;
      if (await over()) tally.overflowScreens++;
      const [n, off, gaps] = await rail();
      tally.hops += n;
      tally.railOff += off;
      tally.railGaps += gaps;
    }
  }

  r.results = { tally, errors: page.errors };
  r.expect(page.errors.length === 0, 'page errors: ' + page.errors.join(' | '));
  r.expect(tally.overflowScreens === 0, 'overflowScreens=' + tally.overflowScreens);
  r.expect(tally.emptyTranscripts === 0, 'emptyTranscripts=' + tally.emptyTranscripts);
  r.expect(tally.railOff === 0, 'railOff=' + tally.railOff);
  r.expect(tally.railGaps === 0, 'railGaps=' + tally.railGaps);
  r.expect(tally.lanes > 0, 'no lanes visited');
  r.expect(tally.turns > 0, 'no turns found');
  r.expect(tally.traces > 0, 'no traces opened');
  r.expect(tally.hops > 0, 'no hops found in any opened trace');

  await page.context().close();
  return r.done();
}

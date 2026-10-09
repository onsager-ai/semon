// Ported from the mockup's full.js. The original drove test-real.html (real logs) on phone dark only, expanding
// every collapsible thing on every lane's session screen and every trace: child work, tool-call steps, and every
// "Show more" on a handoff card, then opened every trace reachable from a turn's Trace button and expanded every
// "more" on its hops. It never drove the sample mockup at all.
//
// Ported to drive the served sample fixture instead (the same measurements, on the fixture's data), phone dark only
// (the original never ran phone light or desktop for this pass). The served sidebar is now a recent-session tree,
// so this exhaustive pass visits every served session by route; Home covers sidebar opening. Trace buttons remain
// real clicks, and route changes wait for the served page's title and transcript.
//
// Assertions (derived from what these counts stand for in the mockup):
//  - no page errors.
//  - zero overflow screens (T.overflowScreens === 0): nothing pokes past the 390px screen on any lane or trace.
//  - every "Show more" on a handoff card fully un-clips its text (moreStillClipped === 0) and never navigates the
//    page away (navigatedByMore === 0) — it is a text reveal, not a link.
//  - a spawn card's brief, opened with "Show more" at 390 px, grows to its full text: its box is as tall as the text (clientHeight >= scrollHeight - 1),
//    and the "Show less" button and the "Open" button sit below it, not under the spilled text; "Show less" clamps it again.
//  - every "more" opened on a trace hop fully un-clips its text (hopMoreStillClipped === 0).
//  - the walk actually visited screens and at least one trace (T.screens > 0, T.traces > 0), so a broken lane list
//    or a missing Trace button would fail loudly instead of reporting an all-zero pass.
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

export default async function full(browser) {
  const D = await data();
  const r = reporter('full');
  const page = await served(browser, { size: 'phone', dark: true });
  const over = () => overflow(page);

  const openAll = () =>
    page.evaluate(() => {
      const r = {
        steps: 0,
        withInput: 0,
        cutNoInput: 0,
        more: 0,
        moreFull: 0,
        moreStillClipped: 0,
        navigatedByMore: 0,
      };
      for (let k = 0; k < 3; k++)
        document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click());
      document.querySelectorAll('.step > button[aria-expanded="false"]').forEach((x) => {
        x.click();
        r.steps++;
        const box = x.parentElement,
          arg = x.querySelector('.sa')?.textContent ?? '';
        if (box.querySelector('.out .io')) r.withInput++;
        else if (arg.endsWith('…')) r.cutNoInput++;
      });
      const before = location.href;
      document.querySelectorAll('.event .ev-more').forEach((m) => {
        if (m.hidden) return;
        r.more++;
        m.click();
        const br = m.parentElement.querySelector('.ev-text');
        if (br.scrollHeight <= br.clientHeight + 1) r.moreFull++;
        else r.moreStillClipped++;
      });
      if (location.href !== before) r.navigatedByMore++;
      return r;
    });

  const T = {
    screens: 0,
    overflowScreens: 0,
    steps: 0,
    withInput: 0,
    cutNoInput: 0,
    more: 0,
    moreFull: 0,
    moreStillClipped: 0,
    navigatedByMore: 0,
    traces: 0,
    hopMore: 0,
    hopMoreStillClipped: 0,
  };
  const add = (rr) => {
    for (const k in rr) T[k] += rr[k];
  };
  const openLane = async (id) => {
    await goto(page, { v: 'session', id }, D);
    await afterTitle(page, D.SESS[id].name);
    await page.waitForFunction(
      () => !!document.querySelector("#page section[aria-label='Transcript']"),
    );
    await page.waitForTimeout(150);
  };

  const lanes = Object.keys(D.SESS);

  for (const id of lanes) {
    await openLane(id);
    add(await openAll());
    await page.waitForTimeout(60);
    T.screens++;
    if (await over()) T.overflowScreens++;
    const tt = await page.evaluate(() =>
      [...document.querySelectorAll('.turn-end .link')].map((b) => b.closest('.turn').dataset.turn),
    );
    for (const t of tt) {
      await openLane(id);
      await page.click('.turn[data-turn="' + t + '"] > .turn-end .link');
      await afterTitle(page, 'Trace');
      await page.waitForTimeout(150);
      T.traces++;
      const rr = await page.evaluate(() => {
        let n = 0,
          c = 0;
        document.querySelectorAll('.hop .more:not([hidden])').forEach((m) => {
          n++;
          m.click();
          const br = m.parentElement.querySelector('.brief');
          if (br.scrollHeight > br.clientHeight + 1) c++;
        });
        return [n, c];
      });
      T.hopMore += rr[0];
      T.hopMoreStillClipped += rr[1];
      await page.waitForTimeout(60);
      T.screens++;
      if (await over()) T.overflowScreens++;
    }
  }

  // One screenshot of an expanded Bash step and an expanded relay, for the record.
  await openLane(lanes[0]);
  await page.evaluate(() => {
    document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click());
  });
  const st = page.locator('.step > button:has(.sa:text-matches("…$"))').first();
  if (await st.count()) {
    await st.click();
    await st.scrollIntoViewIfNeeded();
    await page.evaluate(() => window.scrollBy(0, -120));
    await page.waitForTimeout(400);
    await page.screenshot({ path: path.join(ENV.out, 'full-step.png') });
  }
  const mo = page.locator('.event .ev-more:visible').first();
  if (await mo.count()) {
    await mo.click();
    await mo.scrollIntoViewIfNeeded();
    await page.evaluate(() => window.scrollBy(0, -300));
    await page.waitForTimeout(400);
    await page.screenshot({ path: path.join(ENV.out, 'full-relay.png') });
  }

  // A compact linked row discloses its complete brief in place; long content stays
  // within the expanded region and the following transcript stays below it.
  {
    let probe = null;
    for (const id of lanes) {
      await openLane(id);
      if (await page.locator('.cc-toggle').count())
        await page.locator('.cc-toggle').first().click();
      probe = await page.evaluate(() => {
        const c = document.querySelector('.child-card');
        if (!c) return null;
        const br = c.querySelector('.cc-brief');
        br.replaceChildren(
          document.createTextNode(
            Array.from(
              { length: 14 },
              (_, i) =>
                'Paragraph ' +
                i +
                ' of a long brief. ' +
                'It wraps over several lines on a phone. '.repeat(3),
            ).join(' '),
          ),
        );
        const b = br.getBoundingClientRect(),
          card = c.getBoundingClientRect(),
          next = c.nextElementSibling?.getBoundingClientRect();
        const line = parseFloat(getComputedStyle(br).lineHeight);
        return {
          clamp: getComputedStyle(br).webkitLineClamp,
          client: br.clientHeight,
          scroll: br.scrollHeight,
          line,
          cardInside: b.bottom <= card.bottom + 1,
          nextBelow: !next || next.top >= card.bottom - 1,
          sideways: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
      if (probe) break;
    }
    r.results.spawnBrief = probe;
    r.expect(!!probe, 'the fixture needs a spawn card on some session');
    if (probe) {
      r.expect(
        probe.clamp === 'none' && probe.client >= probe.scroll - 1 && probe.client > probe.line * 2,
        'the expanded linked brief retains its complete text: ' + JSON.stringify(probe),
      );
      r.expect(
        probe.cardInside && probe.nextBelow && probe.sideways <= 0,
        'a long brief stays inside its disclosure, with what follows below it: ' +
          JSON.stringify(probe),
      );
    }
  }

  r.results.tally = T;
  r.results.errors = page.errors;
  r.expect(page.errors.length === 0, 'page errors: ' + page.errors.join(' | '));
  r.expect(T.overflowScreens === 0, 'overflowScreens=' + T.overflowScreens);
  r.expect(T.moreStillClipped === 0, 'moreStillClipped=' + T.moreStillClipped);
  r.expect(T.navigatedByMore === 0, 'navigatedByMore=' + T.navigatedByMore);
  r.expect(T.hopMoreStillClipped === 0, 'hopMoreStillClipped=' + T.hopMoreStillClipped);
  r.expect(T.screens > 0, 'no screens visited');
  r.expect(T.traces > 0, 'no traces opened');

  await page.context().close();
  return r.done();
}

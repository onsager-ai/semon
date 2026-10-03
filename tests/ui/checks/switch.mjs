// Switching sessions paints at once (#29 item 2a), on the primary fixture, phone and desktop. The sidebar's sessions are
// opened by clicking their rows, with /api/tx held back by a route where a check needs the response to be late:
//   - within one frame of a click (at most 50 ms), the row is current and the top bar names the new session; until 150 ms the old
//     page stays, dimmed and inert with aria-busy, and no skeleton is drawn;
//   - with /api/tx delayed by a second, the skeleton shows (six turn-shaped placeholders, aria-hidden, no sideways overflow),
//     then the transcript replaces it with the top bar where it was, aria-busy off and focus on the session's title;
//   - a fast double switch (A, then B while A's response is late) ends on B: A's response is dropped and never flashes;
//   - going back to a session (A, B, A) with no model update in between draws from what the page still holds, a frame after the
//     click, with the top bar and sidebar first: the click's own task is short, and nothing is requested for the session;
//   - after a model update has pruned it, a session left a moment ago draws the same way from the transcript cache;
//   - Back and Forward do the same; a cached session whose growth mark has moved asks for its tail (/api/tx?after=);
//   - a session whose transcript fails to load shows the reason and a way to try again in place of the skeleton;
//   - the cache is bounded: after seven sessions the first is gone (it waits for the network again), a recent one is not.
// Skeleton screenshots at 390 and 1280, light and dark, go to out/switch-skeleton-*.png.
import path from 'node:path';
import { ENV, served, data, reporter, overflow } from '../lib.mjs';

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const plain = (text) =>
  String(text ?? '')
    .replace(/ /g, '')
    .replace(/\s+/g, ' ')
    .trim();
const HELD = 1000;
// A cached session draws on the frame after the click (the sidebar and top bar first), so a frame or two, not the network's 1.5 s.
const HIT_MS = 100;

// The page with /api/tx held back per session (`delays`: sid, or "*" for any) and every request noted in `seen`.
async function open(browser, opts) {
  const page = await served(browser, opts);
  page.delays = new Map();
  page.seen = [];
  page.after = [];
  page.fail = new Set();
  page.bump = new Set();
  await page.route('**/api/tx*', async (route) => {
    const query = new URL(route.request().url()).searchParams,
      sid = query.get('sid');
    const ms = page.delays.get(sid) ?? page.delays.get('*') ?? 0;
    page.seen.push(sid);
    if (query.has('after')) page.after.push(sid);
    if (page.fail.has(sid))
      return route.fulfill({ status: 500, contentType: 'text/plain', body: 'held' });
    if (ms) await sleep(ms);
    try {
      await route.continue();
    } catch {} // the page may have cancelled the request meanwhile
  });
  // A model update on demand. The page keeps every transcript it has loaded until a model update says which ones are still on
  // screen, so the cache is only asked once one has come: the next poll is answered with the real model under a new version.
  page.force = false;
  page.forced = 0;
  await page.route('**/api/model*', async (route) => {
    const url = new URL(route.request().url());
    if (!page.force || !url.searchParams.has('since')) return route.continue();
    page.force = false;
    page.forced++;
    const response = await route.fetch({ url: url.origin + url.pathname }),
      body = await response.json();
    body.version = 'forced-' + page.forced;
    // A session named in `bump` looks as if its transcript had grown: its mark moves on (until the next poll says otherwise).
    for (const sid of page.bump) {
      const [n, bytes] = String(body.tx?.[sid]).split('.').map(Number);
      if (Number.isFinite(n) && Number.isFinite(bytes)) body.tx[sid] = n + 1 + '.' + (bytes + 1);
    }
    page.bump.clear();
    await route.fulfill({ response, json: body });
  });
  return page;
}

// Waits for the page's next poll to be answered with an update, and for the page to have applied it.
async function update(page) {
  page.force = true;
  for (let i = 0; i < 80 && page.force; i++) await sleep(100);
  await sleep(400);
}

// Clicks a row and waits, frame by frame, until the page shows that session's transcript (not busy, its title as the heading),
// up to `limit` ms. Also reports the click's own task (`syncMs`), whether the page was already busy when it ended (`busyAtClick`:
// the page is built after the top bar and sidebar are drawn) and what the top bar said then. Used where the network is held: what
// draws in that time came from what the page already held.
const drawn = (page, id, name, limit) =>
  page.evaluate(
    ([id, name, limit]) =>
      new Promise((resolve) => {
        const plain = (t) =>
            String(t ?? '')
              .replace(/\u2009/g, '')
              .replace(/\s+/g, ' ')
              .trim(),
          main = document.querySelector('#page');
        const row = [...document.querySelectorAll('#lanes .srow[data-id]')].find(
            (x) => x.dataset.id === id,
          ),
          t0 = performance.now();
        row.click();
        const syncMs = performance.now() - t0,
          busyAtClick = main.getAttribute('aria-busy') === 'true',
          bar = plain(document.querySelector('#topbar .t')?.textContent);
        const look = () => {
          const ms = performance.now() - t0;
          if (
            main.getAttribute('aria-busy') !== 'true' &&
            !document.querySelector('.skeleton') &&
            plain(main.querySelector('.ph h1')?.textContent) === name
          )
            return resolve({ drawn: true, ms, syncMs, busyAtClick, bar });
          if (ms > limit)
            return resolve({
              drawn: false,
              ms,
              syncMs,
              busyAtClick,
              bar,
              busy: main.getAttribute('aria-busy') === 'true',
            });
          requestAnimationFrame(look);
        };
        requestAnimationFrame(look);
      }),
    [id, name, limit],
  );

// The sidebar's session rows, top to bottom (the drawer holds them on a phone, hidden but in the page).
const rowsOf = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll('#lanes .srow[data-id]')].map((x) => x.dataset.id),
  );

// Clicks a row and reads the page in the same task, so what it reports is what the click drew synchronously.
const click = (page, id) =>
  page.evaluate((id) => {
    const row = [...document.querySelectorAll('#lanes .srow[data-id]')].find(
        (x) => x.dataset.id === id,
      ),
      t0 = performance.now();
    row.click();
    const ms = performance.now() - t0,
      main = document.querySelector('#page');
    return {
      ms,
      current: document.querySelector('#lanes .srow[aria-current="page"]')?.dataset.id ?? null,
      title: document.querySelector('#topbar .t')?.textContent ?? null,
      busy: main.getAttribute('aria-busy') === 'true',
      inert: main.inert,
      dimmed: main.classList.contains('loading'),
      skeleton: document.querySelectorAll('.skeleton').length,
      transcript: !!main.querySelector("section[aria-label='Transcript']"),
      heading: main.querySelector('.ph h1')?.textContent ?? null,
    };
  }, id);

const landed = (page, id, name) =>
  page.waitForFunction(
    ([id, name]) => {
      const main = document.querySelector('#page'),
        plain = (t) =>
          String(t ?? '')
            .replace(/\u2009/g, '')
            .replace(/\s+/g, ' ')
            .trim();
      return (
        !document.querySelector('.skeleton') &&
        main.getAttribute('aria-busy') !== 'true' &&
        !!main.querySelector("section[aria-label='Transcript']") &&
        plain(main.querySelector('.ph h1')?.textContent) === name &&
        location.pathname.endsWith('/' + encodeURIComponent(id))
      );
    },
    [id, name],
    { timeout: 10_000 },
  );

const barBox = (page) =>
  page.evaluate(() => {
    const bar = document.querySelector('#topbar').getBoundingClientRect(),
      t = document.querySelector('#topbar .t').getBoundingClientRect();
    return {
      barH: Math.round(bar.height * 10) / 10,
      top: Math.round(t.top * 10) / 10,
      left: Math.round(t.left * 10) / 10,
      w: Math.round(t.width * 10) / 10,
    };
  });

export default async function switchCheck(browser) {
  const D = await data();
  const r = reporter('switch');
  const R = r.results;
  const nameOf = (id) => plain(D.SESS[id]?.name);

  // ---- One screen: the first-frame paint, the skeleton, the landing --------------------------------------------------
  async function screen(size, dark, full) {
    const tag = size + (dark ? '-dark' : '-light'),
      out = (R[tag] = {});
    const page = await open(browser, { size, dark });
    const rows = await rowsOf(page);
    r.expect(
      rows.length >= 7,
      tag + ': the sidebar needs at least 7 session rows, has ' + rows.length,
    );
    if (rows.length < 7) {
      await page.context().close();
      return;
    }
    const [a, b] = rows;
    await click(page, a);
    await landed(page, a, nameOf(a));

    // The response is held for a second: what the click draws at once, at 30 ms, and once the skeleton is up.
    page.delays.set(b, HELD);
    const first = await click(page, b);
    await sleep(30);
    const early = await page.evaluate(() => ({
      skeleton: document.querySelectorAll('.skeleton').length,
      busy: document.querySelector('#page').getAttribute('aria-busy') === 'true',
    }));
    out.first = { ...first, ms: Math.round(first.ms * 10) / 10 };
    out.early = early;
    r.expect(
      first.current === b,
      tag + ': the clicked row is not current right after the click: ' + first.current,
    );
    r.expect(
      plain(first.title) === nameOf(b),
      tag + ': the top bar does not name the new session right after the click: ' + first.title,
    );
    r.expect(
      first.ms <= 50,
      tag + ': the click took ' + first.ms + ' ms to draw the row and the top bar (limit 50)',
    );
    r.expect(
      first.busy && first.inert && first.dimmed && first.transcript && first.skeleton === 0,
      tag +
        ': before 150 ms the old page should stay, dimmed, inert and aria-busy, with no skeleton: ' +
        JSON.stringify(first),
    );
    r.expect(
      early.busy && early.skeleton === 0,
      tag + ': a skeleton was drawn before 150 ms: ' + JSON.stringify(early),
    );
    await page.waitForSelector('.skeleton', { state: 'attached', timeout: 3000 });
    const bar1 = await barBox(page);
    const skeleton = await page.evaluate(() => {
      const boxes = [...document.querySelectorAll('.skeleton')],
        main = document.querySelector('#page');
      return {
        count: boxes.length,
        hidden: boxes.every((x) => x.getAttribute('aria-hidden') === 'true'),
        turns: document.querySelectorAll('.skeleton .sk-turn').length,
        busy: main.getAttribute('aria-busy') === 'true',
        oldTranscript: !!main.querySelector("section[aria-label='Transcript']"),
      };
    });
    out.skeleton = skeleton;
    r.expect(
      skeleton.count === 1 &&
        skeleton.hidden &&
        skeleton.turns === 6 &&
        skeleton.busy &&
        !skeleton.oldTranscript,
      tag +
        ': the skeleton is not one aria-hidden block of six turns replacing the old transcript: ' +
        JSON.stringify(skeleton),
    );
    out.overflowSkeleton = await overflow(page);
    r.expect(
      out.overflowSkeleton === 0,
      tag + ': the skeleton overflows sideways: ' + out.overflowSkeleton,
    );
    await page.screenshot({ path: path.join(ENV.out, 'switch-skeleton-' + tag + '.png') });
    await landed(page, b, nameOf(b));
    const bar2 = await barBox(page),
      after = await page.evaluate(() => ({
        active: document.activeElement?.tagName ?? null,
        heading: document.activeElement?.textContent ?? null,
        busy: document.querySelector('#page').hasAttribute('aria-busy'),
        inert: document.querySelector('#page').inert,
        skeleton: document.querySelectorAll('.skeleton').length,
      }));
    out.bar = [bar1, bar2];
    out.after = after;
    r.expect(
      JSON.stringify(bar1) === JSON.stringify(bar2),
      tag + ': the top bar moved when the transcript landed: ' + JSON.stringify([bar1, bar2]),
    );
    r.expect(
      after.active === 'H1' && plain(after.heading) === nameOf(b),
      tag +
        ": focus did not move to the session's title once the transcript landed: " +
        JSON.stringify(after),
    );
    r.expect(
      !after.busy && !after.inert && after.skeleton === 0,
      tag +
        ': the page is still busy, inert or showing a skeleton after landing: ' +
        JSON.stringify(after),
    );
    out.overflowLanded = await overflow(page);
    r.expect(
      out.overflowLanded === 0,
      tag + ': the page overflows sideways after landing: ' + out.overflowLanded,
    );
    if (!full) {
      r.expect(page.errors.length === 0, tag + ': page errors: ' + page.errors.join(' | '));
      await page.context().close();
      return;
    }

    // ---- A fast double switch: A's response is late and must be dropped ------------------------------------------------
    const [c, d] = [rows[2], rows[3]];
    page.delays.clear();
    page.delays.set(c, 1500);
    await click(page, c);
    await sleep(100);
    await click(page, d);
    await landed(page, d, nameOf(d));
    await sleep(1800); // past the moment C's response would have arrived
    out.double = await page.evaluate(() => ({
      path: location.pathname,
      title: document.querySelector('#topbar .t')?.textContent ?? null,
      heading: document.querySelector('#page .ph h1')?.textContent ?? null,
      skeleton: document.querySelectorAll('.skeleton').length,
      current: document.querySelector('#lanes .srow[aria-current="page"]')?.dataset.id ?? null,
    }));
    r.expect(
      out.double.path.endsWith('/' + encodeURIComponent(d)) &&
        plain(out.double.title) === nameOf(d) &&
        plain(out.double.heading) === nameOf(d) &&
        out.double.current === d &&
        out.double.skeleton === 0,
      tag +
        ': a fast double switch did not end on the second session: ' +
        JSON.stringify(out.double),
    );

    // ---- A, B, A with no model update in between: what the page still holds, with the network held ----------------------------
    // (Only a model update prunes a loaded transcript, so this is the common case: nothing is writing, or a poll answered 304.)
    page.delays.clear();
    page.delays.set('*', 1500);
    page.seen.length = 0;
    const warm = await drawn(page, a, nameOf(a), HIT_MS);
    out.warm = {
      ...warm,
      ms: Math.round(warm.ms * 10) / 10,
      syncMs: Math.round(warm.syncMs * 10) / 10,
      updates: page.forced,
    };
    r.expect(
      page.forced === 0,
      tag + ': a model update came before the A, B, A switch, so it did not test what it should',
    );
    r.expect(
      warm.drawn && warm.ms <= HIT_MS,
      tag +
        ': going back to a loaded session did not draw within ' +
        HIT_MS +
        ' ms with the network held: ' +
        JSON.stringify(out.warm),
    );
    r.expect(
      warm.busyAtClick && warm.syncMs <= 50 && plain(warm.bar) === nameOf(a),
      tag +
        ': the click that went back to a loaded session built its page in its own task, or did not draw the top bar first: ' +
        JSON.stringify(out.warm),
    );
    await sleep(300);
    r.expect(
      !page.seen.includes(a),
      tag + ': going back to a loaded session asked /api/tx for its own transcript',
    );

    // ---- Back to a session left a moment ago (a): from the cache once a model update has pruned it -----------------------------
    page.delays.clear();
    await click(page, c);
    await landed(page, c, nameOf(c)); // leave A for another
    await update(page);
    page.delays.set('*', 1500);
    page.seen.length = 0;
    const hit = await drawn(page, a, nameOf(a), HIT_MS);
    out.hit = { ...hit, ms: Math.round(hit.ms * 10) / 10 };
    r.expect(
      hit.drawn,
      tag +
        ': a session left a moment ago did not draw from the cache within ' +
        HIT_MS +
        ' ms with the network held: ' +
        JSON.stringify(hit),
    );
    r.expect(
      out.hit.ms <= HIT_MS,
      tag + ': the cached session took ' + out.hit.ms + ' ms to draw (limit ' + HIT_MS + ')',
    );
    out.hitState = await page.evaluate(() => ({
      current: document.querySelector('#lanes .srow[aria-current="page"]')?.dataset.id ?? null,
      title: document.querySelector('#topbar .t')?.textContent ?? null,
    }));
    r.expect(
      out.hitState.current === a && plain(out.hitState.title) === nameOf(a),
      tag + ": the cached session's row or top bar is wrong: " + JSON.stringify(out.hitState),
    );
    await sleep(400);
    r.expect(
      !page.seen.includes(a),
      tag +
        ': the cached session asked /api/tx for its own transcript although nothing had changed',
    );
    await sleep(1600); // child work held for 1.5 s arrives and the page is drawn again; nothing may break
    out.afterHit = await page.evaluate(() => ({
      heading: document.querySelector('#page .ph h1')?.textContent ?? null,
      transcript: !!document.querySelector("#page section[aria-label='Transcript']"),
      skeleton: document.querySelectorAll('.skeleton').length,
    }));
    r.expect(
      plain(out.afterHit.heading) === nameOf(a) &&
        out.afterHit.transcript &&
        !out.afterHit.skeleton,
      tag +
        ': the cached session broke when its child work arrived: ' +
        JSON.stringify(out.afterHit),
    );

    // ---- Back and Forward, with the network held: the sessions come from the cache --------------------------------------------
    const step = async (dir, id) => {
      const t0 = Date.now();
      await (dir === 'back' ? page.goBack() : page.goForward());
      const ok = await page
        .waitForFunction(
          ([id, name]) => {
            const main = document.querySelector('#page'),
              plain = (t) =>
                String(t ?? '')
                  .replace(/\u2009/g, '')
                  .replace(/\s+/g, ' ')
                  .trim();
            return (
              main.getAttribute('aria-busy') !== 'true' &&
              !document.querySelector('.skeleton') &&
              plain(main.querySelector('.ph h1')?.textContent) === name &&
              location.pathname.endsWith('/' + encodeURIComponent(id))
            );
          },
          [id, nameOf(id)],
          { timeout: 700, polling: 'raf' },
        )
        .then(
          () => true,
          () => false,
        );
      return { ok, ms: Date.now() - t0 };
    };
    // The history so far ends ... a, c, a: Back leads to c, Forward to a again.
    out.back = await step('back', c);
    out.forward = await step('forward', a);
    r.expect(
      out.back.ok,
      tag +
        ': Back did not draw the session before within 700 ms with the network held: ' +
        JSON.stringify(out.back),
    );
    r.expect(
      out.forward.ok,
      tag +
        ': Forward did not draw the session after within 700 ms with the network held: ' +
        JSON.stringify(out.forward),
    );

    // ---- A cached session whose growth mark has moved asks for its tail --------------------------------------------------------
    const [x, y] = [rows[4], rows[5]];
    page.delays.clear();
    page.after.length = 0;
    await click(page, x);
    await landed(page, x, nameOf(x));
    await click(page, y);
    await landed(page, y, nameOf(y));
    page.bump.add(x);
    await update(page); // the next poll is answered with a model where x has grown; x is now cached only
    page.delays.set('*', 1500);
    const grown = await drawn(page, x, nameOf(x), HIT_MS);
    await sleep(2200); // the tail (held for 1.5 s) arrives and the page is drawn again
    out.grown = {
      drawn: grown.drawn,
      ms: Math.round(grown.ms * 10) / 10,
      asked: page.after.filter((sid) => sid === x).length,
      after: await page.evaluate(() => ({
        heading: document.querySelector('#page .ph h1')?.textContent ?? null,
        transcript: !!document.querySelector("#page section[aria-label='Transcript']"),
        skeleton: document.querySelectorAll('.skeleton').length,
      })),
    };
    r.expect(
      grown.drawn && grown.ms <= HIT_MS,
      tag +
        ': a cached session whose mark moved did not draw at once: ' +
        JSON.stringify(out.grown),
    );
    r.expect(
      out.grown.asked >= 1,
      tag + ': a cached session whose mark moved did not ask for its tail',
    );
    r.expect(
      plain(out.grown.after.heading) === nameOf(x) &&
        out.grown.after.transcript &&
        !out.grown.after.skeleton,
      tag + ': the session broke when its tail arrived: ' + JSON.stringify(out.grown),
    );

    // ---- A transcript that fails to load: the reason and a way to try again, in place of the skeleton ------------------------
    const z = rows[6];
    page.delays.clear();
    page.fail.add(z);
    await click(page, z);
    await page.waitForSelector('.load-error', { state: 'attached', timeout: 3000 });
    out.failed = await page.evaluate(() => {
      const main = document.querySelector('#page'),
        box = main.querySelector('.load-error');
      return {
        role: box?.getAttribute('role'),
        text: box?.textContent ?? '',
        button: !!box?.querySelector('button'),
        skeleton: document.querySelectorAll('.skeleton').length,
        busy: main.hasAttribute('aria-busy'),
        inert: main.inert,
        title: document.querySelector('#topbar .t')?.textContent ?? null,
      };
    });
    r.expect(
      out.failed.role === 'alert' &&
        /Couldn't load this session/.test(out.failed.text) &&
        out.failed.button &&
        !out.failed.skeleton &&
        !out.failed.busy &&
        !out.failed.inert &&
        plain(out.failed.title) === nameOf(z),
      tag +
        ': a failed load did not show its reason and a retry button in place of the skeleton: ' +
        JSON.stringify(out.failed),
    );
    out.overflowFailed = await overflow(page);
    r.expect(
      out.overflowFailed === 0,
      tag + ': the failure notice overflows sideways: ' + out.overflowFailed,
    );
    page.fail.delete(z);
    await page.click('.load-error button');
    await landed(page, z, nameOf(z));
    r.expect(page.errors.length === 0, tag + ': page errors: ' + page.errors.join(' | '));
    await page.context().close();
  }

  // ---- The cache is bounded: seven sessions, then the first is gone and a recent one is not -------------------------------
  async function bounded(size) {
    const tag = 'bounded-' + size,
      out = (R[tag] = {});
    const page = await open(browser, { size });
    const rows = (await rowsOf(page)).slice(0, 7);
    r.expect(rows.length === 7, tag + ': the sidebar needs 7 session rows, has ' + rows.length);
    if (rows.length < 7) {
      await page.context().close();
      return;
    }
    for (const id of rows) {
      await click(page, id);
      await landed(page, id, nameOf(id));
    }
    await update(page);
    page.delays.set('*', 1500);
    // The five kept are rows 2 to 6 (the seventh is open): the first is out, and so, once the first is opened, is the second.
    // With the network held for 1.5 s, a session is drawn within 400 ms only if it came from the cache.
    const oldest = await drawn(page, rows[0], nameOf(rows[0]), 400);
    await landed(page, rows[0], nameOf(rows[0]));
    const recent = await drawn(page, rows[5], nameOf(rows[5]), 400);
    const second = await drawn(page, rows[1], nameOf(rows[1]), 400);
    out.drawn = { oldest, recent, second };
    r.expect(!oldest.drawn, tag + ': the first of seven sessions was still in the cache');
    r.expect(
      recent.drawn && recent.ms <= HIT_MS,
      tag + ': a recently left session was not drawn from the cache: ' + JSON.stringify(recent),
    );
    r.expect(!second.drawn, tag + ': the cache kept more than five sessions');
    r.expect(page.errors.length === 0, tag + ': page errors: ' + page.errors.join(' | '));
    await page.context().close();
  }

  await screen('phone', false, true);
  await screen('desktop', false, true);
  await screen('phone', true, false);
  await screen('desktop', true, false);
  await bounded('desktop');
  return r.done();
}

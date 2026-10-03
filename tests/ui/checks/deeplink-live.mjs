// A window opened around an early turn keeps its entries and scroll while the unseen tail grows.
import { served, data, goto, reporter } from '../lib.mjs';
export default async function deepLinkLive(browser) {
  const D = await data(),
    r = reporter('deeplink-live'),
    results = {};
  const sid = 'harbor',
    turn = D.model.turns.find((t) => t.sid === sid).id;
  for (const size of ['phone', 'desktop']) {
    const page = await served(browser, { size });
    let grown = false;
    const requests = [];
    const rows = Array.from({ length: 81 }, (_, slot) => ({
      k: 'a',
      text: 'Entry ' + slot,
      slot,
      ...(slot === 0 ? { turn } : {}),
    }));
    await page.route(/\/api\/tx\?/, async (route) => {
      const q = new URL(route.request().url()).searchParams;
      if (q.get('sid') !== sid || q.has('errors')) return route.continue();
      requests.push(q.toString());
      const total = grown ? 81 : 80;
      const from = q.has('after')
        ? Number(q.get('after'))
        : q.has('turn')
          ? 0
          : Math.max(0, total - 20);
      const to = q.has('turn') ? 20 : total;
      return route.fulfill({
        json: { sid, from, to, total, calls: 0, errors: 0, entries: rows.slice(from, to) },
      });
    });
    await page.route(/\/api\/model(\?|$)/, async (route) => {
      const url = new URL(route.request().url());
      if (grown) url.searchParams.set('since', '');
      const response = await route.fetch({ url: url.toString() });
      if (response.status() !== 200) return route.fulfill({ response });
      const m = await response.json();
      if (grown) {
        m.version += '-deep-grow';
        const [a, b, ...rest] = String(m.tx[sid]).split('.');
        m.tx[sid] = [Number(a) + 1, Number(b) + 1, ...rest].join('.');
      }
      return route.fulfill({ response, json: m });
    });
    await goto(page, { v: 'session', id: sid, turn }, D);
    await page.waitForFunction(() => document.querySelectorAll('.msg.assistant').length === 20);
    await page.evaluate(() => {
      window.__firstDeepRow = document.querySelector('.msg.assistant');
    });
    const before = await page.locator('.msg.assistant').allTextContents();
    grown = true;
    await page.waitForFunction(
      () => document.querySelector('#jump-bottom .new-count')?.textContent === '1 new',
      null,
      { timeout: 15000 },
    );
    r.expect(
      JSON.stringify(await page.locator('.msg.assistant').allTextContents()) ===
        JSON.stringify(before),
      size + ': unseen entries do not replace the deep-link window',
    );
    r.expect(
      await page.evaluate(() => window.__firstDeepRow === document.querySelector('.msg.assistant')),
      size + ': the existing row node remains in place',
    );
    r.expect(
      requests.some((q) => q.includes('after=80')),
      size + ': the metadata request starts beyond the original end',
    );
    await page.locator('#jump-bottom').click();
    await page.waitForFunction(
      () =>
        document.querySelector('.msg.assistant')?.textContent === 'Entry 61' &&
        [...document.querySelectorAll('.msg.assistant')].at(-1)?.textContent === 'Entry 80',
    );
    r.expect(
      (await page.locator('.msg.assistant').count()) === 20,
      size + ': Jump to bottom loads a bounded newest window',
    );
    r.expect(page.errors.length === 0, size + ': page errors: ' + page.errors.join(' | '));
    results[size] = {
      requests,
      initial: before.length,
      latest: await page.locator('.msg.assistant').count(),
    };
    await page.context().close();
  }
  r.results = results;
  return r.done();
}

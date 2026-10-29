// Source-qualified switching keeps late reads isolated and the selected cache bounded.
import assert from 'node:assert/strict';
import { served, data, goto, closePage, overflow } from '../lib.mjs';

export default async function switchCheck(browser) {
  const D = await data(),
    ids = Object.keys(D.SESS).slice(0, 9);
  assert.ok(ids.length >= 9);
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
      const page = await served(browser, {
        size: width === 390 ? 'phone' : 'desktop',
        dark,
        viewport: { width, height: 860 },
      });
      const held = [],
        seen = [],
        retired = [];
      let delay = null,
        fail = null;
      try {
        page.on('request', (request) => {
          if (/\/api\/(model|tx|events)(?:[?/#]|$)/.test(request.url()))
            retired.push(request.url());
        });
        await page.route('**/api/session-transcript?**', async (route) => {
          const url = new URL(route.request().url()),
            sid = url.searchParams.get('sid');
          seen.push({
            sid,
            source: url.searchParams.get('machine'),
            scope: url.searchParams.get('scope'),
          });
          if (sid === fail)
            return route.fulfill({
              status: 503,
              json: {
                api: 1,
                error: {
                  code: 'observation_unavailable',
                  message: 'Selected source is unavailable; retry',
                  retryable: true,
                },
              },
            });
          const response = await route.fetch();
          if (sid === delay) await new Promise((resolve) => held.push(resolve));
          await route.fulfill({ response }).catch(() => {});
        });
        await goto(page, { v: 'session', id: ids[0] }, D);
        const cached = await page.locator('[aria-label="Transcript"]').elementHandle();
        delay = ids[1];
        const slow = goto(page, { v: 'session', id: ids[1] }, D).catch(() => {});
        for (let n = 0; !held.length && n < 100; n++) await page.waitForTimeout(20);
        assert.ok(held.length, 'the qualified late read is actually held');
        await goto(page, { v: 'session', id: ids[2] }, D);
        delay = null;
        held.splice(0).forEach((resolve) => resolve());
        await slow;
        assert.equal(await page.locator('#topbar .t').textContent(), D.SESS[ids[2]].name);
        await goto(page, { v: 'session', id: ids[0] }, D);
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('[aria-label="Transcript"]'),
            cached,
          ),
          true,
          'recent selected views retain their reading root',
        );
        for (const id of ids.slice(3, 8)) await goto(page, { v: 'session', id }, D);
        const count = seen.filter((row) => row.sid === ids[0]).length;
        await goto(page, { v: 'session', id: ids[0] }, D);
        assert.ok(
          seen.filter((row) => row.sid === ids[0]).length > count,
          'the sixth distinct selection evicts the oldest view',
        );
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('[aria-label="Transcript"]'),
            cached,
          ),
          false,
        );
        fail = ids[8];
        await page.evaluate(
          ({ id, machine }) => {
            history.pushState({ v: 'session', id }, '', '/session/' + machine + '/' + id);
            dispatchEvent(new PopStateEvent('popstate'));
          },
          { id: fail, machine: seen[0].source },
        );
        await page
          .getByRole('button', { name: 'Retry selected transcript', exact: false })
          .waitFor();
        fail = null;
        await page.getByRole('button', { name: 'Retry selected transcript', exact: false }).click();
        await page.locator('[aria-label="Transcript"] [data-entry-key]').first().waitFor();
        assert.ok(seen.every((row) => row.source && row.scope === 'retained_history'));
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        held.splice(0).forEach((resolve) => resolve());
        await closePage(page);
      }
    }
  console.log('Canonical selected switching passed');
  return true;
}

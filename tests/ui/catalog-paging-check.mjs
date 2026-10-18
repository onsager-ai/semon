// Real native ranges retain qualified identity and never grow a workspace model.
import assert from 'node:assert/strict';
import { served, data, goto, closePage, overflow } from './lib.mjs';

export async function catalogPagingCheck(browser) {
  const D = await data({ extras: true });
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
      const page = await served(browser, {
        extras: true,
        size: width === 390 ? 'phone' : 'desktop',
        dark,
        viewport: { width, height: 860 },
      });
      const reads = [],
        retired = [];
      try {
        page.on('request', (request) => {
          const url = new URL(request.url());
          if (url.pathname === '/api/session-transcript') reads.push(url);
          if (/^\/api\/(model|tx|events)(?:\/|$)/.test(url.pathname)) retired.push(url.pathname);
        });
        await goto(page, { v: 'session', id: 'backlog' }, D);
        await page.locator('[data-entry-key]').first().waitFor({ state: 'attached' });
        for (let index = 0; index < 3; index++) {
          const button = page.getByRole('button', { name: 'Load earlier records', exact: true });
          if (!(await button.count())) break;
          const before = reads.length;
          const fetched = page.waitForResponse(
            (response) => new URL(response.url()).pathname === '/api/session-transcript',
          );
          await button.click();
          await fetched;
          await page.waitForFunction(() => !document.querySelector('.turns button.more[disabled]'));
          assert.ok(reads.length > before, 'earlier paging issues a selected request');
          assert.ok(
            (await page.locator('[data-entry-key]').count()) <= 300,
            'five retained 60-entry windows bound rendered records',
          );
        }
        const qualified = reads.filter((url) => url.searchParams.get('sid') === 'backlog');
        assert.ok(qualified.length >= 2);
        const source = qualified[0].searchParams.get('machine');
        assert.ok(source);
        assert.ok(
          qualified.every(
            (url) =>
              url.searchParams.get('machine') === source &&
              url.searchParams.get('scope') === 'retained_history' &&
              Number(url.searchParams.get('limit')) <= 100,
          ),
        );
        const ranged = qualified.find(
          (url) => url.searchParams.has('after') && url.searchParams.has('generation'),
        );
        assert.ok(ranged, 'later requests retain the exact generation');
        const wrong = new URL(ranged);
        wrong.searchParams.set('generation', '0'.repeat(64));
        const root = await page.locator('[aria-label="Transcript"]').elementHandle();
        assert.equal((await page.request.get(wrong.href)).status(), 409);
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('[aria-label="Transcript"]'),
            root,
          ),
          true,
        );
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
}

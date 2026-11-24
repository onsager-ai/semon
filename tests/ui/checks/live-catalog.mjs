// Real native source publication updates the canonical selected reader without a model poller.
import assert from 'node:assert/strict';
import { nativeFixture } from '../native-fixture.mjs';
import { closePage, overflow } from '../lib.mjs';

export default async function liveCatalog(browser) {
  const fixture = await nativeFixture();
  try {
    const rebuilding = await fixture.open(browser, { size: 'phone', dark: false });
    try {
      let done = 0;
      const routePattern = '**/api/sessions?*';
      await rebuilding.route(routePattern, (route) =>
        route.fulfill({
          status: 503,
          headers: { 'Retry-After': '1' },
          json: { error: { code: 'history_rebuilding', progress: { done, total: 3 } } },
        }),
      );
      await rebuilding.goto(fixture.base + '/sessions');
      await rebuilding
        .getByRole('status')
        .filter({ hasText: 'Rebuilding history (0 of 3 sources)' })
        .first()
        .waitFor();
      done = 2;
      await rebuilding
        .getByRole('status')
        .filter({ hasText: 'Rebuilding history (2 of 3 sources)' })
        .first()
        .waitFor();
      await rebuilding.unroute(routePattern);
      await rebuilding
        .getByRole('status')
        .filter({ hasText: 'Rebuilding history (2 of 3 sources)' })
        .first()
        .waitFor({ state: 'hidden' });
      assert.deepEqual(rebuilding.errors, []);
    } finally {
      await closePage(rebuilding);
    }
    const selectedRecovery = await fixture.open(browser, { size: 'phone', dark: false });
    try {
      const routePattern = '**/api/session-transcript?*';
      await selectedRecovery.route(routePattern, (route) =>
        route.fulfill({
          status: 503,
          headers: { 'Retry-After': '1' },
          json: {
            error: {
              code: 'history_rebuilding',
              message: 'Rebuilding history (2 of 3 sources)',
              retryable: true,
              progress: { done: 2, total: 3 },
            },
          },
        }),
      );
      await selectedRecovery.goto(fixture.base + '/session/local/harbor');
      await selectedRecovery.getByText('Rebuilding history (2 of 3 sources)').first().waitFor();
      await selectedRecovery.unroute(routePattern);
      await selectedRecovery
        .locator('[aria-label="Transcript"] [data-entry-key]')
        .first()
        .waitFor();
      assert.deepEqual(selectedRecovery.errors, []);
    } finally {
      await closePage(selectedRecovery);
    }
    for (const width of [390, 820, 1280])
      for (const dark of [false, true]) {
        const page = await fixture.open(browser, {
          size: width === 390 ? 'phone' : 'desktop',
          dark,
          viewport: { width, height: 860 },
        });
        const reads = [],
          retired = [];
        let inFlight = 0,
          max = 0;
        const pending = new Set();
        page.on('request', (request) => {
          const url = new URL(request.url());
          if (/^\/api\/(model|tx|events)(?:\/|$)/.test(url.pathname)) retired.push(url.pathname);
          if (url.pathname === '/api/session-transcript') {
            reads.push(url);
            pending.add(request);
            max = Math.max(max, ++inFlight);
          }
        });
        const done = (request) => {
          if (pending.delete(request)) inFlight--;
        };
        page.on('requestfinished', done);
        page.on('requestfailed', done);
        page.on('response', (response) => {
          // A rejected generation has no native content read; its response body is cancelled.
          if (!response.ok()) done(response.request());
        });
        try {
          await page.goto(fixture.base + '/session/local/harbor');
          await page.locator('[aria-label="Transcript"] [data-entry-key]').first().waitFor();
          const root = await page.locator('[aria-label="Transcript"]').elementHandle();
          await page.waitForTimeout(200);
          const idleReads = reads.length;
          await page.waitForTimeout(3500);
          assert.equal(
            reads.length,
            idleReads,
            'unchanged selected observations do not transfer transcript bodies',
          );
          const marker = `Native live publication ${width} ${dark}`;
          fixture.append('harbor', marker);
          await page.clock.setFixedTime(fixture.now + 60000);
          await page.getByText(marker, { exact: true }).waitFor({ timeout: 30000 });
          assert.equal(
            await page.evaluate(
              (node) => node === document.querySelector('[aria-label="Transcript"]'),
              root,
            ),
            true,
            'publication updates the accepted selected root',
          );
          const source = reads[0].searchParams.get('machine');
          assert.ok(source);
          assert.ok(
            reads.every(
              (url) =>
                url.searchParams.get('machine') === source &&
                url.searchParams.get('scope') === 'current' &&
                Number(url.searchParams.get('limit') ?? 32) <= 32,
            ),
          );
          assert.ok(
            max <= 1,
            'one selected request owner admits each range: ' +
              JSON.stringify(
                reads.map((url) => ({
                  sid: url.searchParams.get('sid'),
                  scope: url.searchParams.get('scope'),
                  earlier: url.searchParams.has('cursor'),
                  addressed: url.searchParams.has('address'),
                })),
              ),
          );
          await page.locator('#find-btn').click();
          await page.locator('#find').fill(marker);
          assert.equal(await page.getByText(marker, { exact: true }).count(), 1);
          assert.equal(await page.locator('.fcount').textContent(), '1 loaded match');
          await page.locator('#find').press('Escape');
          await page.evaluate(() => {
            Object.defineProperty(document, 'visibilityState', {
              configurable: true,
              get: () => 'hidden',
            });
            dispatchEvent(new Event('visibilitychange'));
          });
          await page.waitForTimeout(200);
          const hidden = reads.length;
          await page.clock.setFixedTime(fixture.now + 120000);
          await page.waitForTimeout(3500);
          assert.equal(reads.length, hidden, 'hidden documents pause selected reads');
          await page.evaluate(() => {
            delete document.visibilityState;
            document.dispatchEvent(new Event('visibilitychange'));
          });
          const next = marker + ' after focus';
          fixture.append('harbor', next);
          await page.clock.setFixedTime(fixture.now + 180000);
          await page.getByText(next, { exact: true }).waitFor({ timeout: 30000 });
          assert.equal(await overflow(page), 0);
          assert.deepEqual(retired, []);
          assert.deepEqual(page.errors, []);
        } finally {
          await closePage(page);
        }
      }
  } finally {
    await fixture.close();
  }
  console.log('Native publication, selected updates, visibility and request ownership passed');
  return true;
}

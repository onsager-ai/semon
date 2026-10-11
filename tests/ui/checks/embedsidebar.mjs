// A native page adopts the canonical chrome through the typed host. The retired
// sidebar/model application is replaced by an independently built consumer.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { ENV, context, reporter, overflow, closePage } from '../lib.mjs';

export default async function embedSidebar(browser) {
  const report = reporter('embedsidebar');
  const output = path.join(ENV.out, 'embedsidebar');
  fs.mkdirSync(output, { recursive: true });
  const root = new URL('../../../', import.meta.url).pathname;
  const bundle = path.join(output, 'sidebar-host.generated.js');
  execFileSync(
    process.execPath,
    [
      path.join(root, 'ui/build.mjs'),
      '--entry',
      path.join(root, 'tests/ui/sidebar-host.ts'),
      '--output',
      bundle,
    ],
    { cwd: root },
  );
  const html = fs.readFileSync(new URL('../shell-sidebar.html', import.meta.url), 'utf8');
  const script = fs.readFileSync(bundle, 'utf8');
  const results = [];
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
      const ctx = await context(browser, {
        size: width === 390 ? 'phone' : 'desktop',
        dark,
        viewport: { width, height: 860 },
      });
      const page = await ctx.newPage();
      const errors = [],
        retired = [];
      page.on('pageerror', (error) => errors.push(error.message));
      page.on('request', (request) => {
        if (/\/api\/(model|tx)(?:[/?]|$)/.test(new URL(request.url()).pathname))
          retired.push(request.url());
      });
      await page.clock.setFixedTime(ENV.now);
      const sheet = await page.goto(ENV.base + '/shell.css?t=' + ENV.token);
      const headers = Object.fromEntries(
        Object.entries(sheet.headers()).filter(
          ([name]) =>
            ![
              'content-type',
              'content-length',
              'content-encoding',
              'etag',
              'set-cookie',
              'date',
              'connection',
              'transfer-encoding',
              'keep-alive',
            ].includes(name),
        ),
      );
      await page.route(ENV.base + '/sidebar-host.js', (route) =>
        route.fulfill({
          status: 200,
          headers,
          contentType: 'text/javascript',
          body: script,
        }),
      );
      await page.route(ENV.base + '/__shell-sidebar.html', (route) =>
        route.fulfill({
          status: 200,
          headers,
          contentType: 'text/html',
          body: html,
        }),
      );
      await page.goto(ENV.base + '/__shell-sidebar.html');
      await page.waitForSelector('#lanes .srow', { state: 'attached' });
      await page.evaluate(() => document.fonts.ready);
      const own = await page.evaluate((html) => {
        const was = new DOMParser().parseFromString(html, 'text/html');
        return {
          untouched:
            was.querySelector('#page').innerHTML === document.querySelector('#page').innerHTML,
          title: document.querySelector('#topbar .t').textContent,
          current: [...document.querySelectorAll('#nav [aria-current="page"]')].map(
            (row) => row.dataset.go,
          ),
          qualified: [...document.querySelectorAll('#lanes .srow')].every((row) => {
            const id = JSON.parse(row.dataset.id);
            return (
              id.length === 2 && id.every((value) => typeof value === 'string' && value.length)
            );
          }),
        };
      }, html);
      assert.deepEqual(own, {
        untouched: true,
        title: 'Devices',
        current: ['machines'],
        qualified: true,
      });
      assert.equal(await overflow(page), 0);
      if (width === 390) {
        await page.click('#lead-btn');
        await page.waitForFunction(() => document.body.classList.contains('drawer-open'));
        await page.keyboard.press('Escape');
        await page.waitForFunction(() => !document.body.classList.contains('drawer-open'));
        assert.equal(await page.evaluate(() => document.activeElement.id), 'lead-btn');
        await page.click('#lead-btn');
      }
      await page.screenshot({
        path: path.join(output, width + '-' + (dark ? 'dark' : 'light') + '.png'),
      });
      const [source] = JSON.parse(
        await page.locator('#lanes .srow').first().getAttribute('data-id'),
      );
      await page.evaluate(() => {
        window.__nativePageMarker = true;
      });
      await page.locator('#lanes .srow').first().click();
      await page.waitForURL((url) => url.pathname.startsWith('/s/'));
      assert.equal(
        await page.evaluate(() => window.__nativePageMarker),
        undefined,
        'native rows retain server navigation',
      );
      assert.equal(new URL(page.url()).searchParams.get('machine'), source);
      assert.deepEqual(errors, []);
      assert.deepEqual(retired, []);
      results.push({ width, theme: dark ? 'dark' : 'light', ...own, nativeNavigation: true });
      await closePage(page);
    }
  report.results = results;
  return report.done();
}

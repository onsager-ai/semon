// Canonical Sessions use bounded source metadata and native query controls.
import assert from 'node:assert/strict';
import path from 'node:path';
import { ENV, served, overflow, closePage } from './lib.mjs';

export async function catalogFilterCheck(browser, name) {
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
      const page = await served(browser, {
        size: width === 390 ? 'phone' : 'desktop',
        dark,
        viewport: { width, height: 860 },
        path: '/sessions',
      });
      try {
        const requests = [],
          retired = [];
        page.on('request', (request) => {
          const url = new URL(request.url());
          if (url.pathname === '/api/sessions') requests.push(url);
          if (/^\/api\/(model|tx|events)(?:\/|$)/.test(url.pathname)) retired.push(url.pathname);
        });
        await page.getByLabel('Harness', { exact: true }).waitFor();
        await page.locator('#page .nrow').first().waitFor();
        const capabilities = await (
          await page.request.get(ENV.base + '/api/session-capabilities')
        ).json();
        const source = new URL(page.url()).searchParams.get('machine') ?? capabilities.source_key;
        assert.ok(source, 'list URL retains the selected machine');
        const response = await page.request.get(
          ENV.base +
            '/api/sessions?' +
            new URLSearchParams({ machine: source, scope: 'retained_history', limit: '60' }),
        );
        assert.equal(response.status(), 200);
        const all = await response.json(),
          row = all.items.find((row) => row.harness && row.repo);
        assert.ok(row, 'fixture includes a recorded repository');
        await page.getByLabel('Harness', { exact: true }).fill(row.harness);
        await page.getByLabel('Repository', { exact: true }).fill(row.repo);
        await page.getByRole('button', { name: 'Apply filters', exact: true }).press('Enter');
        const expected = all.items
          .filter((item) => item.harness === row.harness && item.repo === row.repo)
          .map((item) => item.key)
          .sort();
        await page.waitForFunction(
          (ids) =>
            JSON.stringify(
              [...document.querySelectorAll('#page .nrow')].map((row) => row.dataset.id).sort(),
            ) === JSON.stringify(ids),
          expected,
        );
        const url = new URL(page.url());
        assert.equal(url.searchParams.get('harness'), row.harness);
        assert.equal(url.searchParams.get('repo'), row.repo);
        assert.equal(url.searchParams.get('machine'), source);
        assert.equal(await overflow(page), 0);
        await page.screenshot({
          path: path.join(ENV.out, `${name}-sessions-${width}-${dark ? 'dark' : 'light'}.png`),
        });
        await page.reload();
        await page.getByLabel('Harness', { exact: true }).waitFor();
        assert.equal(await page.getByLabel('Harness', { exact: true }).inputValue(), row.harness);
        assert.equal(await page.getByLabel('Repository', { exact: true }).inputValue(), row.repo);
        await page.getByLabel('Repository', { exact: true }).fill('');
        await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
        await page.waitForFunction(() => !new URLSearchParams(location.search).has('repo'));
        assert.equal(new URL(page.url()).searchParams.get('harness'), row.harness);
        assert.ok(requests.some((url) => url.searchParams.get('harness') === row.harness));
        assert.ok(
          requests.every(
            (url) =>
              url.searchParams.get('machine') === source &&
              url.searchParams.get('scope') === 'retained_history' &&
              Number(url.searchParams.get('limit') ?? 60) <= 100,
          ),
        );
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
}

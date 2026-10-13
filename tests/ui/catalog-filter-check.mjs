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
        const observed = all.items.find((item) => item.status?.observation === 'current');
        assert.ok(observed, 'fixture includes qualified current status');
        let pendingInjected = false,
          refreshed = false;
        const pendingFacts = async (route) => {
          const response = await route.fetch(),
            body = await response.json();
          if (!pendingInjected && body.completeness.state === 'complete') {
            const item = body.items.find((item) => item.key === observed.key);
            assert.ok(item, 'qualified status remains in the same bounded page');
            item.status = { state: null, observation: 'unavailable' };
            item.facts_observation = { state: dark ? 'unavailable' : 'stale' };
            body.freshness = dark ? 'cached' : 'stale';
            pendingInjected = true;
            await route.fulfill({ response, json: body });
          } else {
            refreshed ||= pendingInjected;
            await route.fulfill({ response });
          }
        };
        await page.route('**/api/sessions?*', pendingFacts);
        try {
          await page.reload();
          await page.waitForFunction(
            ({ key, state }) =>
              document.querySelector(`.nrow[data-id="${key}"] .dot`)?.classList.contains(state),
            { key: observed.key, state: observed.status.state },
          );
          assert.ok(pendingInjected && refreshed, 'complete pages retry pending runtime facts');
        } finally {
          await page.unroute('**/api/sessions?*', pendingFacts);
        }
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
        const noRepository = page.getByRole('checkbox', { name: 'No repository', exact: true });
        const geometry = await noRepository.evaluate((checkbox) => {
          const box = checkbox.getBoundingClientRect(),
            label = checkbox.parentElement.querySelector('span').getBoundingClientRect();
          return { width: box.width, gap: label.left - box.right };
        });
        assert.ok(geometry.width < 32, 'checkbox retains its native width');
        assert.equal(geometry.gap, 8, 'checkbox stays beside its associated label');
        await noRepository.check();
        await page.waitForFunction(
          () => new URLSearchParams(location.search).get('no_repo') === '1',
        );
        assert.equal(new URL(page.url()).searchParams.get('harness'), row.harness);
        assert.equal(new URL(page.url()).searchParams.has('repo'), false);
        const withoutRepository = all.items
          .filter((item) => item.harness === row.harness && !item.repo)
          .map((item) => item.key)
          .sort();
        await page.waitForFunction(
          (ids) =>
            JSON.stringify(
              [...document.querySelectorAll('#page .nrow')].map((row) => row.dataset.id).sort(),
            ) === JSON.stringify(ids),
          withoutRepository,
        );
        await noRepository.uncheck();
        await page.waitForFunction(() => !new URLSearchParams(location.search).has('no_repo'));
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

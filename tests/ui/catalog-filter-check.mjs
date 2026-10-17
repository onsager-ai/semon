// Canonical Sessions use bounded source metadata and native query controls.
import assert from 'node:assert/strict';
import path from 'node:path';
import {
  ENV,
  served,
  overflow,
  closePage,
  filterSheet,
  doneFilterSheet,
  pickFilter,
} from './lib.mjs';

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
        await page.locator('#sq').waitFor();
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
        await filterSheet(page);
        for (const [label, value] of [
          ['Harness', row.harness],
          ['Repo', row.repo],
        ]) {
          const select = page.locator(`.filters-sheet .sh-select[data-label="${label}"]`);
          await select.locator('.sh-select-trigger').click();
          const index = await select
            .locator('[role="option"]')
            .evaluateAll(
              (nodes, value) => nodes.findIndex((node) => node.dataset.value === value),
              value,
            );
          assert.ok(index >= 0, 'recorded filter option is available');
          await select.locator('[role="option"]').nth(index).click();
        }
        await doneFilterSheet(page);
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
        await page.locator('#sq').waitFor();
        await filterSheet(page);
        assert.equal(
          await page
            .locator('.sh-select[data-label="Harness"] [role="option"][aria-selected="true"]')
            .getAttribute('data-value'),
          row.harness,
        );
        assert.equal(
          await page
            .locator('.sh-select[data-label="Repo"] [role="option"][aria-selected="true"]')
            .getAttribute('data-value'),
          row.repo,
        );
        await doneFilterSheet(page);
        await pickFilter(page, 'Repo', '');
        await page.waitForFunction(() => !new URLSearchParams(location.search).has('repo'));
        assert.equal(new URL(page.url()).searchParams.get('harness'), row.harness);
        await pickFilter(page, 'Repo', '__none__');
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
        await pickFilter(page, 'Repo', '');
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

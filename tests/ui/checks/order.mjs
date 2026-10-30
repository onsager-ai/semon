// Stable visible order uses real native publication and the canonical Source snapshots.
import assert from 'node:assert/strict';
import { nativeFixture } from '../native-fixture.mjs';
import { logs } from '../native-order-logs.mjs';
import { fileURLToPath } from 'node:url';
import { closePage, overflow, launch } from '../lib.mjs';
import { selectSchemes } from '../suite-plan.mjs';

const rows = (page, selector) =>
  page.locator(selector).evaluateAll((nodes) =>
    nodes.map((node) => ({
      id: node.dataset.id,
      top: node.getBoundingClientRect().top,
    })),
  );
const ids = (values) => values.map((value) => value.id);
const still = (before, after) =>
  before.every((value) => {
    const next = after.find((row) => row.id === value.id);
    return next && Math.abs(value.top - next.top) <= 1;
  });
export default async function orderCheck(browser) {
  const schemes = [390, 820, 1280].flatMap((width) =>
    [false, true].map((dark) => [
      (width === 390 ? 'phone' : width === 820 ? 'tablet' : 'desktop') +
        '-' +
        (dark ? 'dark' : 'light'),
      { width, dark },
    ]),
  );
  for (const [, { width, dark }] of selectSchemes(schemes, process.env.SEMON_ORDER_SCHEMES)) {
    const fixture = await nativeFixture({
      prepare: (root, now) => {
        const files = logs(root);
        for (let index = 0; index < 24; index++)
          files.create('native-order-seed-' + index, now - index * 10000 - 100000);
      },
    });
    const page = await fixture.open(browser, {
      size: width === 390 ? 'phone' : 'desktop',
      dark,
      viewport: { width, height: 420 },
      init: null,
    });
    const files = logs(fixture.root),
      retired = [];
    try {
      await page.addInitScript(() => {
        window.__semonOrderIdleMs = 600;
      });
      await page.goto(fixture.base + '/sessions');
      await page.waitForFunction(
        () => document.querySelectorAll('.session-list .nrow').length >= 24,
      );
      page.on('request', (request) => {
        if (/^\/api\/(model|tx|events)(?:\/|$)/.test(new URL(request.url()).pathname))
          retired.push(request.url());
      });
      const source = (await (await fixture.request('/api/session-capabilities')).json()).source_key;
      const catalog =
        '/api/sessions?' +
        new URLSearchParams({ machine: source, scope: 'retained_history', limit: '60' });
      let revision = 0;
      const published = async (key) => {
        let latest;
        for (let attempt = 0; attempt < 200; attempt++) {
          const value = await (await fixture.request(catalog)).json();
          latest = value;
          if (value.items?.[0]?.key === key) return;
          await page.waitForTimeout(50);
        }
        assert.fail(
          'Native publication did not admit ' +
            key +
            '\nCatalog: ' +
            JSON.stringify({
              generation: latest?.generation,
              completeness: latest?.completeness,
              refresh: latest?.refresh,
              first: latest?.items?.[0]?.key,
              error: latest?.error,
            }) +
            '\nNative fixture output:\n' +
            fixture.diagnostics(),
        );
      };
      const drawn = async () => {
        const previous = revision;
        await page.waitForFunction((previous) => window.__nativeOrderDraws > previous, previous, {
          timeout: 15000,
        });
        revision = await page.evaluate(() => window.__nativeOrderDraws);
        await page.waitForTimeout(100);
      };
      await page.evaluate(() => {
        window.__nativeOrderDraws = 0;
      });
      page.on('response', async (response) => {
        const url = new URL(response.url());
        if (url.pathname === '/api/sessions' && !url.searchParams.has('sid') && response.ok()) {
          await page
            .evaluate(() => {
              window.__nativeOrderDraws++;
            })
            .catch(() => {});
        }
      });
      await page.evaluate(() => {
        const scroll = matchMedia('(max-width: 760px)').matches
          ? document.scrollingElement
          : document.querySelector('#main');
        const first = document.querySelector('.session-list .nrow').getBoundingClientRect();
        const top = document.querySelector('#topbar').getBoundingClientRect().bottom;
        scroll.scrollTop += first.top - top + 100;
      });
      await page.mouse.move(width - 8, 4);
      const before = await rows(page, '.session-list .nrow');
      const newId = 'native-order-new';
      files.bump('harbor', fixture.now + 1000);
      files.create(newId, fixture.now + 2000);
      await published(newId);
      await drawn();
      const held = await rows(page, '.session-list .nrow');
      assert.deepEqual(
        ids(held),
        ids(before),
        'scrolled visible rows hold recency changes and new sessions',
      );
      assert.ok(still(before, held), 'row coordinates and scroll stay fixed');
      assert.equal(await page.locator('.order-pill, .order-chip, #order-status').count(), 0);
      if (width === 390) await page.locator('#topbar .lead').click();
      await page.locator('.nav-item[data-go="analytics"]').click();
      await page
        .locator('h1')
        .filter({ hasText: /^Analytics$/ })
        .waitFor();
      if (width === 390) await page.locator('#topbar .lead').click();
      await page.locator('.nav-item[data-go="sessions"]').click();
      await page.locator('.session-list .nrow').first().waitFor();
      assert.equal(
        (await rows(page, '.session-list .nrow'))[0].id,
        newId,
        'navigation applies held Source order',
      );
      if (width === 390) {
        await page.locator('#topbar .lead').click();
        await page.locator('#drawer-close').waitFor();
        const roots = await rows(page, '#lanes > .treeitem');
        assert.equal(JSON.parse(roots[0].id)[1], newId, 'opening drawer applies hidden order');
        await page.evaluate(() => {
          document.querySelector('#side-list').scrollTop = 60;
        });
        const openBefore = await rows(page, '#lanes > .treeitem');
        files.create('native-order-drawer', fixture.now + 4000);
        await published('native-order-drawer');
        await drawn();
        assert.deepEqual(ids(await rows(page, '#lanes > .treeitem')), ids(openBefore));
        await page.locator('#drawer-close').click();
        assert.equal(
          JSON.parse((await rows(page, '#lanes > .treeitem'))[0].id)[1],
          'native-order-drawer',
        );
      } else {
        const first = page.locator('#lanes > .treeitem > .tree-row .srow').first();
        await first.focus();
        await page.keyboard.press('Tab');
        const sideBefore = await rows(page, '#lanes > .treeitem');
        const focused = await page.locator(':focus').elementHandle();
        files.create('native-order-sidebar', fixture.now + 4000);
        await published('native-order-sidebar');
        await drawn();
        await page.waitForTimeout(800);
        assert.deepEqual(
          ids(await rows(page, '#lanes > .treeitem')),
          ids(sideBefore),
          'keyboard focus holds sidebar past idle',
        );
        assert.equal(await page.evaluate((node) => node === document.activeElement, focused), true);
        await page.locator('#sq').focus();
        await page.mouse.move(width - 8, 4);
        await page.waitForTimeout(800);
        assert.equal(
          JSON.parse((await rows(page, '#lanes > .treeitem'))[0].id)[1],
          'native-order-sidebar',
          'idle applies held sidebar order',
        );
      }
      await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
      await page.waitForTimeout(100);
      assert.equal(await overflow(page), 0);
      assert.deepEqual(retired, []);
      assert.deepEqual(page.errors, []);
    } finally {
      await closePage(page);
      await fixture.close();
    }
  }
  console.log('Native visible order, navigation, drawer, idle and keyboard focus passed');
  return true;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  try {
    await orderCheck(browser);
  } finally {
    await browser.close();
  }
}

// Recent sessions use compact source-qualified Home rows and the shared tree/sheet.
import assert from 'node:assert/strict';
import { served, data, goto, closePage, overflow } from '../lib.mjs';

export default async function sidebarCheck(browser) {
  const D = await data();
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
      const page = await served(browser, {
        size: width === 390 ? 'phone' : 'desktop',
        dark,
        viewport: { width, height: 860 },
      });
      const retired = [];
      try {
        page.on('request', (request) => {
          if (/\/api\/(model|tx|events)(?:[?/#]|$)/.test(request.url()))
            retired.push(request.url());
        });
        await page.locator('#lanes .treeitem').first().waitFor({ state: 'attached' });
        if (width === 390)
          await page.getByRole('button', { name: 'Open menu', exact: true }).click();
        const root = page.locator('#lanes > .treeitem').filter({
          has: page.locator(':scope > .tree-row > .srow .nm', { hasText: /^atlas ingest$/ }),
        });
        assert.equal(await root.count(), 1);
        const sourceKey = JSON.parse(await root.getAttribute('data-id'))[0];
        assert.equal((await page.locator('#lanes > .treeitem').count()) <= 8, true);
        for (const id of await page
          .locator('#lanes .srow')
          .evaluateAll((nodes) => nodes.map((n) => n.dataset.id))) {
          const key = JSON.parse(id);
          assert.equal(key.length, 2);
          assert.ok(key.every((part) => typeof part === 'string' && part.length));
        }
        const toggle = root.locator(':scope > .tree-row .tree-toggle');
        if ((await root.getAttribute('aria-expanded')) !== 'true') await toggle.click();
        const all = root.locator(':scope > .tree-group > .tree-all');
        await all.waitFor();
        const before = page.url();
        await all.click();
        if (width === 390) {
          const sheet = page.getByRole('dialog', {
            name: 'All sessions under atlas ingest',
            exact: true,
          });
          await sheet.waitFor();
          assert.ok((await sheet.locator('.kids-row').count()) >= 6);
          assert.equal(await page.evaluate(() => history.state?.sheet), 1);
          await sheet
            .getByRole('searchbox', { name: 'Search these sessions', exact: true })
            .fill('parse_frame');
          assert.equal(await sheet.locator('.kids-row').count(), 1);
          assert.match(await sheet.locator('.kids-row').textContent(), /Find parse_frame callers/);
          await page.goBack();
          await sheet.waitFor({ state: 'detached' });
          assert.equal(page.url(), before);
          assert.equal(await all.evaluate((node) => document.activeElement === node), true);
          await all.click();
          await sheet.waitFor();
          await sheet
            .getByRole('button', {
              name: 'Find parse_frame callers, Recorded history',
              exact: true,
            })
            .click();
          await page.locator('[aria-label="Transcript"] [data-entry-key]').first().waitFor();
          assert.equal(new URL(page.url()).searchParams.get('machine'), sourceKey);
          assert.equal(await page.locator('.kids-sheet').count(), 0);
          assert.match(await page.locator('#topbar .t').textContent(), /Find parse_frame callers/);
        } else {
          assert.ok((await root.locator(':scope > .tree-group > .treeitem').count()) >= 6);
          const fewer = root.getByRole('button', {
            name: 'Show fewer sessions under atlas ingest',
            exact: true,
          });
          await fewer.click();
          assert.equal(await all.count(), 1);
          assert.equal(
            await root
              .locator(':scope > .tree-row > .srow')
              .evaluate((node) => document.activeElement === node),
            true,
          );
          await root.focus();
          await page.keyboard.press('ArrowLeft');
          assert.equal(await root.getAttribute('aria-expanded'), 'false');
          await page.keyboard.press('ArrowRight');
          assert.equal(await root.getAttribute('aria-expanded'), 'true');
          await goto(page, { v: 'session', id: 'harbor' }, D);
          assert.equal(await page.locator('#lanes .srow[aria-current="page"]').count(), 1);
          const selected = JSON.parse(
            await page.locator('#lanes .srow[aria-current="page"]').getAttribute('data-id'),
          );
          assert.deepEqual(selected, [sourceKey, 'harbor']);
        }
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
  console.log('Canonical Recent tree, mobile sheet, focus and source navigation passed');
  return true;
}

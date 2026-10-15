// The shared session Topbar presents source metadata, indexed annotations and one panel lifecycle.
import assert from 'node:assert/strict';
import { served, data, goto, closePage, overflow } from '../lib.mjs';

export default async function barCheck(browser) {
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
          if (/\/api\/(model|tx|events|control|session-control)(?:[?/#]|$)/.test(request.url()))
            retired.push(request.url());
        });
        await goto(page, { v: 'session', id: 'harbor' }, D);
        assert.equal(await page.locator('#topbar .t').textContent(), 'harbor');
        assert.equal(await page.locator('#topbar #find-btn').count(), 1);
        assert.equal(await page.locator('#topbar #more-btn').count(), 1);
        assert.equal(await page.locator('#topbar .session-composer-trigger').count(), 0);
        assert.equal(await page.locator('#page [aria-label="Conversation controls"]').count(), 0);
        assert.equal(await overflow(page), 0);
        const trigger = page.locator('#more-btn');
        await trigger.click();
        const menu = page.getByRole('dialog', { name: 'Session menu for harbor', exact: true });
        await menu.waitFor();
        assert.match(await menu.textContent(), /at API rates/);
        assert.match(await menu.textContent(), /Claude Code/);
        assert.match(await menu.textContent(), /feat\/offline-sync|Runs/);
        const rectangle = await menu.boundingBox();
        assert.ok(rectangle.x >= 0 && rectangle.x + rectangle.width <= width + 1);
        await page.keyboard.press('Escape');
        await menu.waitFor({ state: 'detached' });
        await page.waitForFunction(() => !history.state?.sheet);
        assert.equal(await trigger.evaluate((node) => node === document.activeElement), true);
        const url = page.url();
        await trigger.click();
        await menu.waitFor();
        await page.goBack();
        await menu.waitFor({ state: 'detached' });
        assert.equal(page.url(), url);
        await page.locator('#find-btn').click();
        assert.equal(
          await page.locator('#find').evaluate((node) => node === document.activeElement),
          true,
        );
        await page.locator('#find').fill('ingest');
        assert.match(await page.locator('.fcount').textContent(), /loaded/);
        const modes = page.getByRole('group', { name: 'Show', exact: true });
        await modes.getByRole('button', { name: 'Messages', exact: true }).click();
        assert.equal(await page.locator('.turns .step').count(), 0);
        await modes.getByRole('button', { name: 'Steps', exact: true }).click();
        assert.equal(await page.locator('.turns .msg').count(), 0);
        await modes.getByRole('button', { name: 'All', exact: true }).click();
        assert.equal(await modes.locator('[aria-pressed="true"]').count(), 1);
        await page.locator('#find').press('Escape');
        await page.locator('#find-btn').waitFor();
        await goto(page, { v: 'session', id: 'h-failed' }, D);
        await page.locator('#find-btn').click();
        await page.getByRole('button', { name: /^Failed steps/ }).click();
        await page.locator('.err-current').waitFor();
        assert.match(await page.locator('.errnav-bar').textContent(), /Error 1 of \d+/);
        await page.keyboard.press('Escape');
        assert.equal(await page.locator('.err-current').count(), 0);
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
  console.log('Canonical session Topbar, menus, loaded find and annotations passed');
  return true;
}

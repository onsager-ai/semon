import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';

test('independent shell consumer unmounts roots and releases dismissal listeners', async () => {
  const bundle = await build({
    absWorkingDir: new URL('../', import.meta.url).pathname,
    entryPoints: ['src/lib-contract.tsx'], bundle: true, write: false,
    format: 'iife', globalName: 'AccountExample', platform: 'browser',
    tsconfig: 'tsconfig.json', define: { 'process.env.NODE_ENV': '"production"' },
  });
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.route('http://account.test/**', route => route.fulfill({
      contentType: route.request().url().endsWith('.js') ? 'text/javascript' : 'text/html',
      headers: { 'Content-Security-Policy': "default-src 'self'; script-src 'self'; style-src 'self'" },
      body: route.request().url().endsWith('.js') ? bundle.outputFiles[0].text
        : '<!doctype html><div id="shell"></div><script src="/consumer.js"></script>',
    }));
    await page.goto('http://account.test/');
    const result = await page.evaluate(() => {
      const events = [];
      const container = document.querySelector('#shell');
      const host = { place() {}, opened() { events.push('open'); }, closed() { events.push('close'); }, navigate() { return false; }, submit() { return false; } };
      for (let i = 0; i < 20; i++) {
        const instance = AccountExample.mountAccountExample(container, host);
        const trigger = container.querySelector('.account-trigger');
        trigger.click();
        if (!instance.chrome.open || container.querySelectorAll('.account-popover').length !== 1) throw new Error('mount/open failed');
        instance.destroy();
        trigger.click();
        if (instance.chrome.open || container.childElementCount) throw new Error('destroy left root ownership');
        const count = events.length;
        document.body.click();
        dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
        instance.destroy();
        if (events.length !== count) throw new Error('destroy leaked a listener or was not idempotent');
      }
      const instance = AccountExample.mountAccountExample(container, host);
      const root = container.firstElementChild;
      const trigger = root.querySelector('.account-trigger');
      trigger.click();
      instance.chrome.unmount(root);
      root.remove();
      trigger.click();
      const closed = !instance.chrome.open;
      instance.destroy();
      return { opens: events.filter(e => e === 'open').length, closed };
    });
    assert.equal(result.opens, 21);
    assert.equal(result.closed, true);
  } finally { await browser.close(); }
});

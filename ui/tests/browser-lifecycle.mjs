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
      const listeners = new Map();
      let registrations = 0, removals = 0;
      const add = EventTarget.prototype.addEventListener;
      const remove = EventTarget.prototype.removeEventListener;
      const tracked = (target, type) => (target === document && type === 'click') || (target === window && type === 'pageshow');
      EventTarget.prototype.addEventListener = function(type, callback, options) {
        if (tracked(this, type)) { listeners.set(callback, { target: this, type }); registrations++; }
        return add.call(this, type, callback, options);
      };
      EventTarget.prototype.removeEventListener = function(type, callback, options) {
        if (tracked(this, type)) {
          const entry = listeners.get(callback);
          if (entry?.target !== this || entry?.type !== type) throw new Error('unmatched listener removal');
          listeners.delete(callback); removals++;
        }
        return remove.call(this, type, callback, options);
      };
      try {
        const container = document.querySelector('#shell');
        const host = { place() {}, opened() { events.push('open'); }, closed() { events.push('close'); }, navigate() { return false; }, submit() { return false; } };
        for (let i = 0; i < 20; i++) {
          const instance = AccountExample.mountAccountExample(container, host);
          if (listeners.size !== 2) throw new Error('shell did not register its two dismissal listeners');
          const trigger = container.querySelector('.account-trigger');
          const beforeOpen = events.length;
          trigger.click();
          if (events.length !== beforeOpen + 1 || events.at(-1) !== 'open') throw new Error('open emitted a false close transition');
          if (!instance.chrome.open || container.querySelectorAll('.account-popover').length !== 1) throw new Error('mount/open failed');
          instance.destroy();
          if (listeners.size) throw new Error('destroy left registered dismissal listeners');
          trigger.click();
          if (instance.chrome.open || container.childElementCount) throw new Error('destroy left root ownership');
          const count = events.length;
          document.body.click();
          dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
          instance.destroy();
          instance.chrome.close();
          instance.chrome.escape();
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
        return { opens: events.filter(e => e === 'open').length, closes: events.filter(e => e === 'close').length, closed, registrations, removals, remaining: listeners.size };
      } finally {
        EventTarget.prototype.addEventListener = add;
        EventTarget.prototype.removeEventListener = remove;
      }
    });
    assert.equal(result.opens, 21);
    assert.equal(result.closes, 21);
    assert.equal(result.closed, true);
    assert.equal(result.registrations, 42);
    assert.equal(result.removals, 42);
    assert.equal(result.remaining, 0);
  } finally { await browser.close(); }
});

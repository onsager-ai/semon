import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
import { readFile } from 'node:fs/promises';

for (const width of [390, 1280])
  test(`native shell teardown and remount ${width}`, async () => {
    const contract = await build({
      absWorkingDir: new URL('../', import.meta.url).pathname,
      entryPoints: ['src/app/native-shell.ts'],
      bundle: true,
      write: false,
      format: 'iife',
      globalName: 'NativeShell',
      platform: 'browser',
      tsconfig: 'tsconfig.json',
    });
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({ viewport: { width, height: 860 } });
      await page.addInitScript(() => {
        const listeners = new Set(),
          intervals = new Set(),
          timeouts = new Set();
        const add = EventTarget.prototype.addEventListener,
          remove = EventTarget.prototype.removeEventListener;
        EventTarget.prototype.addEventListener = function (type, fn, opts) {
          listeners.add(fn);
          return add.call(this, type, fn, opts);
        };
        EventTarget.prototype.removeEventListener = function (type, fn, opts) {
          listeners.delete(fn);
          return remove.call(this, type, fn, opts);
        };
        const si = window.setInterval,
          ci = window.clearInterval,
          st = window.setTimeout,
          ct = window.clearTimeout;
        window.setInterval = function (fn, delay) {
          const id = si(fn, delay);
          intervals.add(id);
          return id;
        };
        window.clearInterval = function (id) {
          intervals.delete(id);
          ci(id);
        };
        window.setTimeout = function (fn, delay) {
          const id = st(() => {
            timeouts.delete(id);
            fn();
          }, delay);
          timeouts.add(id);
          return id;
        };
        window.clearTimeout = function (id) {
          timeouts.delete(id);
          ct(id);
        };
        window.resources = () => [listeners.size, intervals.size, timeouts.size];
      });
      const production = await readFile(
        new URL('../../crates/semon-sessions/src/shell.generated.js', import.meta.url),
        'utf8',
      );
      await page.route('http://native.test/**', (route) =>
        route.fulfill({
          contentType: route.request().url().endsWith('.js') ? 'text/javascript' : 'text/html',
          body: route.request().url().endsWith('/contract.js')
            ? contract.outputFiles[0].text
            : route.request().url().endsWith('/production.js')
              ? production
              : '<!doctype html><button id="lead-btn">Drawer</button><aside id="sidebar"><button id="drawer-close">Close</button></aside><div id="scrim"></div><div id="main"></div><div id="topbar"></div><code id="source">literal &lt;img&gt;</code><button data-copy="source">Copy</button><form data-confirm="confirm"><button type="submit">Submit</button></form><dialog id="confirm"><button value="cancel">Cancel</button><button value="confirm">Confirm</button></dialog><span data-poll="/ready"></span><script src="/contract.js"></script>',
        }),
      );
      await page.goto('http://native.test/');
      await page.evaluate(() =>
        document.addEventListener('submit', (event) => event.preventDefault()),
      );
      const result = await page.evaluate(async (width) => {
        const base = resources();
        const outcomes = [];
        for (let i = 0; i < 3; i++) {
          const owner = NativeShell.mountNativeShell();
          document.getElementById('lead-btn').click();
          if (document.body.classList.contains('drawer-open') !== width < 760)
            throw new Error('drawer breakpoint changed');
          const form = document.querySelector('form');
          let submissions = 0;
          const submitted = (e) => {
            submissions++;
            e.preventDefault();
          };
          form.addEventListener('submit', submitted);
          form.requestSubmit();
          if (!document.getElementById('confirm').open)
            throw new Error('confirmation did not open');
          document.querySelector('[value="confirm"]').click();
          if (submissions !== 2 || document.getElementById('confirm').open)
            throw new Error('confirm did not resubmit the original form');
          form.requestSubmit();
          document.querySelector('[value="cancel"]').click();
          if (submissions !== 3 || document.getElementById('confirm').open)
            throw new Error('cancel did not close without resubmitting');
          form.removeEventListener('submit', submitted);
          document.getElementById('topbar').classList.add('scrolled');
          owner.destroy();
          owner.destroy();
          if (document.getElementById('topbar').classList.contains('scrolled'))
            throw new Error('stale scroll chrome survived destroy');
          document.getElementById('lead-btn').click();
          form.requestSubmit();
          if (
            document.body.classList.contains('drawer-open') ||
            document.getElementById('confirm').open
          )
            throw new Error('stale controls revived');
          outcomes.push(resources());
        }
        const controls = NativeShell.mountNativeShell({ chrome: false });
        document.getElementById('lead-btn').click();
        if (document.body.classList.contains('drawer-open'))
          throw new Error('controls-only mode duplicated drawer ownership');
        document.querySelector('form').requestSubmit();
        if (!document.getElementById('confirm').open)
          throw new Error('controls-only mode lost native confirmation');
        document.querySelector('[value="cancel"]').click();
        controls.destroy();
        outcomes.push(resources());
        // A transport/clipboard completion that ignores abort must still be inert after teardown.
        const originalFetch = window.fetch,
          originalInterval = window.setInterval;
        let poll, signal, resolvePoll, resolveCopy, rejectCopy;
        window.setInterval = (fn, delay) => {
          poll = fn;
          return originalInterval(fn, delay);
        };
        window.fetch = (_url, options) => {
          signal = options.signal;
          return new Promise((resolve) => {
            resolvePoll = resolve;
          });
        };
        Object.defineProperty(navigator, 'clipboard', {
          configurable: true,
          value: {
            writeText: () =>
              new Promise((resolve, reject) => {
                resolveCopy = resolve;
                rejectCopy = reject;
              }),
          },
        });
        const pending = NativeShell.mountNativeShell();
        poll();
        document.querySelector('[data-copy]').click();
        if (!signal || signal.aborted) throw new Error('poll has no live owner');
        pending.destroy();
        if (!signal.aborted) throw new Error('readiness request survived teardown');
        resolvePoll({ status: 200 });
        resolveCopy();
        await Promise.resolve();
        await Promise.resolve();
        if (document.querySelector('[data-copy]').classList.contains('copied'))
          throw new Error('stale clipboard callback changed the page');
        outcomes.push(resources());
        const detached = NativeShell.mountNativeShell();
        poll();
        const readiness = document.querySelector('[data-poll]');
        readiness.remove();
        resolvePoll({ status: 200 });
        await Promise.resolve();
        await Promise.resolve();
        if (location.pathname !== '/') throw new Error('detached readiness control navigated');
        detached.destroy();
        document.body.append(readiness);
        window.fetch = originalFetch;
        window.setInterval = originalInterval;
        outcomes.push(resources());
        const stale = NativeShell.mountNativeShell();
        const copy = document.querySelector('[data-copy]');
        copy.click();
        copy.remove();
        const selection = getSelection();
        selection.removeAllRanges();
        rejectCopy(Error('clipboard unavailable'));
        await Promise.resolve();
        await Promise.resolve();
        if (selection.rangeCount) throw new Error('detached copy rejection changed selection');
        stale.destroy();
        document.body.append(copy);
        outcomes.push(resources());
        const active = NativeShell.mountNativeShell();
        const mounted = resources();
        const replaced = NativeShell.mountNativeShell();
        if (resources().some((n, i) => n !== mounted[i]))
          throw new Error('remount duplicated effects');
        active.destroy();
        replaced.destroy();
        outcomes.push(resources());
        return { base, outcomes };
      }, width);
      for (const counts of result.outcomes) assert.deepEqual(counts, result.base);
      // The Rust-served production entry must apply the same native enhancement.
      await page.addScriptTag({ url: 'http://native.test/production.js' });
      await page.locator('form').evaluate((form) => form.requestSubmit());
      await page.waitForFunction(() => document.getElementById('confirm').open);
      await page.locator('[value="cancel"]').click();
      assert.equal(await page.locator('#confirm').evaluate((d) => d.open), false);
    } finally {
      await browser.close();
    }
  });

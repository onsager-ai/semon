import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';

for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark']) {
    test(`independent panel roots, native dismissal and cleanup ${width} ${colorScheme}`, async () => {
      const bundle = await build({
        absWorkingDir: new URL('../', import.meta.url).pathname,
        entryPoints: ['src/lib-contract.tsx'],
        bundle: true,
        write: false,
        format: 'iife',
        globalName: 'AccountExample',
        platform: 'browser',
        tsconfig: 'tsconfig.json',
        define: { 'process.env.NODE_ENV': '"production"' },
      });
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({ viewport: { width, height: 860 }, colorScheme });
        await page.route('http://panel.test/**', (route) =>
          route.fulfill({
            contentType: route.request().url().endsWith('.js') ? 'text/javascript' : 'text/html',
            headers: {
              'Content-Security-Policy': "default-src 'self'; script-src 'self'; style-src 'self'",
            },
            body: route.request().url().endsWith('.js')
              ? bundle.outputFiles[0].text
              : '<!doctype html><meta charset="utf-8"><button id="trigger">Open</button><script src="/consumer.js"></script>',
          }),
        );
        await page.goto('http://panel.test/');
        await page.evaluate(async () => {
          const assert = (condition, message) => {
            if (!condition) throw new Error(message);
          };
          const add = document.addEventListener,
            remove = document.removeEventListener;
          const listeners = new Map();
          let registrations = 0,
            removals = 0;
          document.addEventListener = function (type, callback, options) {
            if (['wheel', 'touchmove'].includes(type)) {
              listeners.set(type, callback);
              registrations++;
            }
            return add.call(this, type, callback, options);
          };
          document.removeEventListener = function (type, callback, options) {
            if (['wheel', 'touchmove'].includes(type) && listeners.get(type) === callback) {
              listeners.delete(type);
              removals++;
            }
            return remove.call(this, type, callback, options);
          };
          try {
            const hostile = '<img src=x onerror=alert(1)>',
              trigger = document.querySelector('#trigger');
            for (const dismissal of ['button', 'backdrop', 'native', 'destroy', 'detached']) {
              let opens = 0,
                closes = 0;
              const host = {
                opened() {
                  assert(this === host, 'lost opened callback context');
                  opens++;
                  assert(
                    panel.dialog.open && document.activeElement === panel.body,
                    'modal/focus was not committed before host callback',
                  );
                },
                closed() {
                  assert(this === host, 'lost closed callback context');
                  closes++;
                  assert(
                    !panel.dialog.isConnected && listeners.size === 0,
                    'closed callback preceded cleanup',
                  );
                  trigger.focus();
                },
              };
              const panel = AccountExample.createPanelChrome(
                { title: hostile, sub: hostile, label: hostile, className: 'full' },
                host,
              );
              const input = document.createElement('input');
              panel.body.append(input);
              const close = panel.dialog.querySelector('.ibtn');
              assert(
                panel.dialog.children.length === 2 &&
                  panel.body.previousElementSibling.className === 'panel-h',
                'host slot topology changed',
              );
              assert(
                !panel.dialog.querySelector('img') &&
                  panel.dialog.querySelector('.panel-t').textContent === hostile &&
                  panel.dialog.querySelector('.panel-sub').textContent === hostile,
                'hostile text became markup',
              );
              panel.show();
              panel.show();
              assert(opens === 1 && listeners.size === 2, 'duplicate modal or scroll listeners');
              for (const type of ['wheel', 'touchmove']) {
                const event = new Event(type, { bubbles: true, cancelable: true });
                panel.dialog.dispatchEvent(event);
                assert(event.defaultPrevented, 'background scroll escaped');
                const inner = new Event(type, { bubbles: true, cancelable: true });
                input.dispatchEvent(inner);
                assert(inner.defaultPrevented, 'non-scrollable body escaped');
                Object.defineProperty(panel.body, 'scrollHeight', {
                  configurable: true,
                  value: 1000,
                });
                Object.defineProperty(panel.body, 'clientHeight', {
                  configurable: true,
                  value: 100,
                });
                const scroll = new Event(type, { bubbles: true, cancelable: true });
                input.dispatchEvent(scroll);
                assert(!scroll.defaultPrevented, 'scrollable host content was blocked');
                delete panel.body.scrollHeight;
                delete panel.body.clientHeight;
              }
              if (dismissal === 'button') close.click();
              else if (dismissal === 'backdrop')
                panel.dialog.dispatchEvent(new MouseEvent('click', { bubbles: true }));
              else if (dismissal === 'native') panel.dialog.close();
              else if (dismissal === 'detached') {
                panel.dialog.remove();
                document.dispatchEvent(new Event('wheel', { cancelable: true }));
              } else panel.destroy();
              await new Promise((resolve) =>
                requestAnimationFrame(() => requestAnimationFrame(resolve)),
              );
              assert(
                closes === 1 && listeners.size === 0 && document.activeElement === trigger,
                'dismissal cleanup/focus failed: ' + dismissal,
              );
              assert(panel.body.firstChild === input, 'Preact mutated host content');
              close.click();
              panel.show();
              panel.destroy();
              panel.dialog.dispatchEvent(new Event('close'));
              assert(
                closes === 1 && opens === 1 && !panel.dialog.open && !panel.dialog.isConnected,
                'disposed controller revived or emitted callbacks',
              );
            }
            const neverShown = AccountExample.createPanelChrome(
              { title: 'Unused' },
              {
                opened() {
                  throw new Error('unexpected open');
                },
                closed() {
                  throw new Error('unexpected close');
                },
              },
            );
            neverShown.destroy();
            neverShown.show();
            assert(
              !neverShown.dialog.isConnected && registrations === removals,
              'unused root or listeners leaked',
            );
            const spaced = AccountExample.createPanelChrome(
              { title: 'One · two', sub: '· Leading' },
              { opened() {}, closed() {} },
            );
            assert(
              spaced.dialog.querySelector('.panel-t').textContent === 'One\u2009 · \u2009two' &&
                spaced.dialog.querySelector('.panel-sub').textContent === '·\u2009 Leading',
              'separator spacing changed: ' +
                JSON.stringify([
                  spaced.dialog.querySelector('.panel-t').textContent,
                  spaced.dialog.querySelector('.panel-sub').textContent,
                ]),
            );
            spaced.destroy();
          } finally {
            document.addEventListener = add;
            document.removeEventListener = remove;
          }
        });
        // Real Escape exercises the browser's native cancel/close path.
        await page.evaluate(() => {
          window.escapePanel = AccountExample.createPanelChrome(
            { title: 'Escape' },
            {
              opened() {},
              closed() {
                window.escapeClosed = true;
              },
            },
          );
          escapePanel.show();
        });
        await page.keyboard.press('Escape');
        await page.waitForFunction(() => window.escapeClosed && !escapePanel.dialog.isConnected);
      } finally {
        await browser.close();
      }
    });
  }

test('independent shell consumer unmounts roots and releases dismissal listeners', async () => {
  const bundle = await build({
    absWorkingDir: new URL('../', import.meta.url).pathname,
    entryPoints: ['src/lib-contract.tsx'],
    bundle: true,
    write: false,
    format: 'iife',
    globalName: 'AccountExample',
    platform: 'browser',
    tsconfig: 'tsconfig.json',
    define: { 'process.env.NODE_ENV': '"production"' },
  });
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.route('http://account.test/**', (route) =>
      route.fulfill({
        contentType: route.request().url().endsWith('.js') ? 'text/javascript' : 'text/html',
        headers: {
          'Content-Security-Policy': "default-src 'self'; script-src 'self'; style-src 'self'",
        },
        body: route.request().url().endsWith('.js')
          ? bundle.outputFiles[0].text
          : '<!doctype html><div id="shell"></div><script src="/consumer.js"></script>',
      }),
    );
    await page.goto('http://account.test/');
    const result = await page.evaluate(() => {
      const events = [];
      const listeners = new Map();
      let registrations = 0,
        removals = 0;
      const add = EventTarget.prototype.addEventListener;
      const remove = EventTarget.prototype.removeEventListener;
      const tracked = (target, type) =>
        (target === document && type === 'click') || (target === window && type === 'pageshow');
      EventTarget.prototype.addEventListener = function (type, callback, options) {
        if (tracked(this, type)) {
          listeners.set(callback, { target: this, type });
          registrations++;
        }
        return add.call(this, type, callback, options);
      };
      EventTarget.prototype.removeEventListener = function (type, callback, options) {
        if (tracked(this, type)) {
          const entry = listeners.get(callback);
          if (entry?.target !== this || entry?.type !== type)
            throw new Error('unmatched listener removal');
          listeners.delete(callback);
          removals++;
        }
        return remove.call(this, type, callback, options);
      };
      try {
        const container = document.querySelector('#shell');
        const host = {
          place() {},
          opened() {
            events.push('open');
          },
          closed() {
            events.push('close');
          },
          navigate() {
            return false;
          },
          submit() {
            return false;
          },
        };
        for (let i = 0; i < 20; i++) {
          const instance = AccountExample.mountAccountExample(container, host);
          if (listeners.size !== 2)
            throw new Error('shell did not register its two dismissal listeners');
          const trigger = container.querySelector('.account-trigger');
          const beforeOpen = events.length;
          trigger.click();
          if (events.length !== beforeOpen + 1 || events.at(-1) !== 'open')
            throw new Error('open emitted a false close transition');
          if (!instance.chrome.open || container.querySelectorAll('.account-popover').length !== 1)
            throw new Error('mount/open failed');
          instance.destroy();
          if (listeners.size) throw new Error('destroy left registered dismissal listeners');
          trigger.click();
          if (instance.chrome.open || container.childElementCount)
            throw new Error('destroy left root ownership');
          const count = events.length;
          document.body.click();
          dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
          instance.destroy();
          instance.chrome.close();
          instance.chrome.escape();
          if (events.length !== count)
            throw new Error('destroy leaked a listener or was not idempotent');
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
        return {
          opens: events.filter((e) => e === 'open').length,
          closes: events.filter((e) => e === 'close').length,
          closed,
          registrations,
          removals,
          remaining: listeners.size,
        };
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
  } finally {
    await browser.close();
  }
});

for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark']) {
    test(`independent application shell ownership/lifecycle ${width} ${colorScheme}`, async () => {
      const bundle = await build({
        absWorkingDir: new URL('../', import.meta.url).pathname,
        entryPoints: ['src/lib-contract.tsx'],
        bundle: true,
        write: false,
        format: 'iife',
        globalName: 'AccountExample',
        platform: 'browser',
        tsconfig: 'tsconfig.json',
        define: { 'process.env.NODE_ENV': '"production"' },
      });
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({ viewport: { width, height: 860 }, colorScheme });
        await page.route('http://shell.test/**', (route) =>
          route.fulfill({
            contentType: route.request().url().endsWith('.js') ? 'text/javascript' : 'text/html',
            headers: {
              'Content-Security-Policy': "default-src 'self'; script-src 'self'; style-src 'self'",
            },
            body: route.request().url().endsWith('.js')
              ? bundle.outputFiles[0].text
              : '<!doctype html><div class="app" id="shell"></div><script src="/consumer.js"></script>',
          }),
        );
        await page.goto('http://shell.test/');
        const result = await page.evaluate(() => {
          const assert = (condition, message) => {
            if (!condition) throw new Error(message);
          };
          const listeners = new Map();
          let registrations = 0,
            removals = 0,
            navigations = 0,
            opened = 0,
            closed = 0,
            railChanges = 0;
          const add = EventTarget.prototype.addEventListener,
            remove = EventTarget.prototype.removeEventListener;
          const tracked = (target, type) =>
            (target === window && type === 'pageshow') ||
            (target === document && type === 'click') ||
            (target instanceof MediaQueryList && type === 'change') ||
            (target instanceof HTMLElement && ['sidebar', 'scrim'].includes(target.id));
          EventTarget.prototype.addEventListener = function (type, callback, options) {
            if (tracked(this, type)) {
              listeners.set(callback, { target: this, type });
              registrations++;
            }
            return add.call(this, type, callback, options);
          };
          EventTarget.prototype.removeEventListener = function (type, callback, options) {
            if (tracked(this, type)) {
              const entry = listeners.get(callback);
              assert(entry?.target === this && entry?.type === type, 'unmatched listener removal');
              listeners.delete(callback);
              removals++;
            }
            return remove.call(this, type, callback, options);
          };
          try {
            const container = document.querySelector('#shell');
            const host = {
              account: {
                place() {},
                opened() {},
                closed() {},
                navigate() {
                  return false;
                },
                submit() {
                  return false;
                },
              },
              navigate() {
                assert(this === host, 'shell lost host callback context');
                navigations++;
                return true;
              },
              drawerOpened() {
                opened++;
              },
              drawerClosed() {
                closed++;
              },
              railChanged() {
                railChanges++;
              },
            };
            const destination = {
              key: 'sessions',
              label: '<img src=x onerror=alert(1)>',
              href: '/sessions',
              icon: 'M4 5h16',
              current: true,
            };
            for (let i = 0; i < 20; i++) {
              const instance = AccountExample.mountShellExample(container, host),
                chrome = instance.chrome;
              const slots = chrome.slots;
              const input = document.createElement('input');
              slots.content.append(input);
              const recent = document.createElement('button');
              slots.recent.append(recent);
              const oldClose = container.querySelector('#drawer-close'),
                oldRail = container.querySelector('#rail-toggle');
              const nav = container.querySelector('.nav-item'),
                lead = container.querySelector('#lead-btn'),
                trigger = container.querySelector('.account-widget-desktop .account-trigger');
              assert(listeners.size === 7, 'missing lifecycle registrations: ' + listeners.size);
              nav.focus();
              chrome.update([destination], true);
              assert(
                container.querySelector('.nav-item') === nav && document.activeElement === nav,
                'keyed navigation lost focus',
              );
              assert(
                !nav.querySelector('img') && nav.textContent === destination.label,
                'unsafe navigation label',
              );
              for (const modifiers of [
                { ctrlKey: true },
                { metaKey: true },
                { shiftKey: true },
                { altKey: true },
                { button: 1 },
              ]) {
                const event = new MouseEvent('click', {
                  bubbles: true,
                  cancelable: true,
                  ...modifiers,
                });
                nav.dispatchEvent(event);
                assert(!event.defaultPrevented, 'modified navigation consumed');
              }
              const click = new MouseEvent('click', { bubbles: true, cancelable: true });
              nav.dispatchEvent(click);
              assert(click.defaultPrevented, 'primary click not consumed');
              const before = nav.getAttribute('href');
              try {
                chrome.update([{ ...destination, href: '//evil.test/' }], false);
                throw new Error('unsafe destination accepted');
              } catch (error) {
                assert(error.message === 'Invalid shell destination', error.message);
              }
              assert(nav.getAttribute('href') === before, 'invalid update mutated navigation');
              lead.click();
              const phone = matchMedia('(max-width: 760px)').matches;
              assert(chrome.drawerOpen === phone, 'responsive drawer');
              chrome.closeDrawer();
              assert(!chrome.drawerOpen, 'drawer close');
              trigger.click();
              assert(chrome.account.open, 'account did not open');
              const action = instance.action;
              action.focus();
              chrome.topbar({ titleSlot: instance.title, actions: [action] });
              assert(
                document.activeElement === action && !chrome.account.open,
                'action slot focus or account teardown',
              );
              chrome.topbar({ mode: [action] });
              assert(action.parentElement.id === 'topbar', 'mode slot was wrapped');
              assert(
                input.parentElement === slots.content && recent.parentElement === slots.recent,
                'host slots changed',
              );
              chrome.unmount();
              assert(listeners.size === 0, 'unmount leaked lifecycle listeners');
              assert(
                action.childNodes.length === 1 &&
                  !action.isConnected &&
                  input.isConnected &&
                  recent.isConnected,
                'unmount cleared host-owned descendants',
              );
              const n = navigations;
              lead.click();
              trigger.click();
              nav.click();
              assert(
                navigations === n && !chrome.account.open && !chrome.drawerOpen,
                'stale chrome remained live',
              );
              chrome.mount(container);
              chrome.update([destination], false);
              chrome.topbar({
                titleSlot: instance.title,
                lead: { label: 'Open navigation', icon: 'M4 7h16' },
              });
              chrome.openDrawer();
              oldClose.click();
              oldRail.click();
              lead.click();
              assert(
                chrome.drawerOpen === phone && railChanges === 0,
                'stale frame controls changed a remounted shell',
              );
              chrome.unmount();
              instance.destroy();
              instance.destroy();
              assert(listeners.size === 0, 'destroy leaked lifecycle listeners');
              input.remove();
              recent.remove();
            }
            return {
              registrations,
              removals,
              remaining: listeners.size,
              navigations,
              opened,
              closed,
            };
          } finally {
            EventTarget.prototype.addEventListener = add;
            EventTarget.prototype.removeEventListener = remove;
          }
        });
        assert.equal(result.registrations, result.removals);
        assert.equal(result.remaining, 0);
        assert.equal(result.navigations, 20);
        assert.equal(result.opened, width === 390 ? 40 : 0);
        assert.equal(result.closed, result.opened);
      } finally {
        await browser.close();
      }
    });
  }

test('Recent commits preserve keyed focus and destroy rejects detached controls', async () => {
  const bundle = await build({
    absWorkingDir: new URL('../', import.meta.url).pathname,
    entryPoints: ['src/lib-contract.tsx'],
    bundle: true,
    write: false,
    format: 'iife',
    globalName: 'AccountExample',
    platform: 'browser',
    tsconfig: 'tsconfig.json',
  });
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.setContent('<div id="recent"></div>');
    await page.addScriptTag({ content: bundle.outputFiles[0].text });
    const result = await page.evaluate(() => {
      const root = document.querySelector('#recent'),
        events = [];
      const item = {
        id: 'parent',
        name: '<img onerror=alert(1)>',
        label: 'Parent',
        state: 'work',
        stateLabel: 'Working',
        age: 'now',
        model: 'Model',
        modelTip: 'Model',
        fields: [],
        rail: false,
        open: true,
        children: [],
        depth: 0,
        all: 9,
        stuck: false,
      };
      const host = {
        open: (id) => events.push(id),
        toggle: (id, open) => events.push(open),
        all: (id) => events.push(id),
        fewer: (id) => events.push(id),
      };
      const snapshot = { items: [item], empty: false };
      const renderer = AccountExample.mountRecentExample(root, host, snapshot),
        row = root.querySelector('.srow');
      row.focus();
      renderer.update({ ...snapshot, items: [{ ...item, age: '1m' }] });
      if (root.querySelector('.srow') !== row || document.activeElement !== row)
        throw new Error('keyed commit lost focus');
      if (root.querySelector('img') || root.querySelector('.nm').textContent !== item.name)
        throw new Error('Recent did not escape text');
      row.click();
      root.querySelector('.tree-toggle').click();
      root.querySelector('.tree-all').click();
      renderer.destroy();
      renderer.destroy();
      row.click();
      window.dispatchEvent(new Event('resize'));
      let rejected = false;
      try {
        renderer.update(snapshot);
      } catch {
        rejected = true;
      }
      return { events, empty: !root.childNodes.length, rejected };
    });
    assert.deepEqual(result, { events: ['parent', false, 'parent'], empty: true, rejected: true });
  } finally {
    await browser.close();
  }
});

test('persistent facet root keeps open controls and releases detached modal listeners', async () => {
  const bundle = await build({
    absWorkingDir: new URL('../', import.meta.url).pathname,
    entryPoints: ['src/lib-contract.tsx'],
    bundle: true,
    write: false,
    format: 'iife',
    globalName: 'AccountExample',
    platform: 'browser',
    tsconfig: 'tsconfig.json',
    define: { 'process.env.NODE_ENV': '"production"' },
  });
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.setContent('<!doctype html><meta charset="utf-8"><main></main>');
    await page.addScriptTag({ content: bundle.outputFiles[0].text });
    await page.evaluate(async () => {
      const assert = (condition, message) => {
        if (!condition) throw new Error(message);
      };
      let created = 0,
        opens = 0;
      const closures = [],
        choices = [];
      const fields = [
        {
          key: 'repo',
          label: 'Repo',
          value: '',
          options: [{ value: '', label: 'All repos' }],
          display: '',
        },
      ];
      const facets = AccountExample.createFacetChrome({
        select(label) {
          created++;
          const el = document.createElement('div'),
            input = document.createElement('input');
          el.append(input);
          return {
            el,
            setOptions() {},
            setValue() {},
            focus() {
              input.focus();
            },
            close() {},
          };
        },
        change() {},
        cleared() {},
        canOpen: () => true,
        opened() {
          opens++;
        },
        closed(dialog, reason) {
          closures.push(reason);
        },
        clear() {},
      });
      facets.update(fields);
      document.querySelector('main').append(facets.element);
      facets.element.querySelector('.facet-btn').click();
      const dialog = facets.element.querySelector('dialog'),
        input = dialog.querySelector('input');
      input.value = 'typing';
      input.setSelectionRange(2, 4);
      facets.update(fields);
      assert(
        dialog.open &&
          input === dialog.querySelector('input') &&
          document.activeElement === input &&
          input.selectionStart === 2 &&
          created === 1,
        'open control identity/caret changed',
      );
      const blocked = new Event('wheel', { cancelable: true });
      document.dispatchEvent(blocked);
      assert(blocked.defaultPrevented, 'modal scroll escaped');
      facets.element.remove();
      const released = new Event('wheel', { cancelable: true });
      document.dispatchEvent(released);
      assert(
        !released.defaultPrevented && closures.join() === 'destroyed',
        'detached modal did not release owner',
      );
      facets.destroy();
      facets.destroy();
      assert(closures.length === 1, 'duplicate destruction callback');
      facets.element.querySelector('.facet-btn')?.click();
      assert(opens === 1, 'destroyed controller revived');
    });
  } finally {
    await browser.close();
  }
});

test('typed markdown treats hostile content as text and preserves nested log structure', async () => {
  const bundle = await build({
    absWorkingDir: new URL('../', import.meta.url).pathname,
    entryPoints: ['src/lib-contract.tsx'],
    bundle: true,
    write: false,
    format: 'iife',
    globalName: 'AccountExample',
    platform: 'browser',
    tsconfig: 'tsconfig.json',
    define: { 'process.env.NODE_ENV': '"production"' },
  });
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.setContent('<!doctype html><meta charset="utf-8"><main></main>');
    await page.addScriptTag({ content: bundle.outputFiles[0].text });
    const result = await page.evaluate(() => {
      const markdown = AccountExample.createMarkdown(
        '<img src=x onerror=alert(1)>\n[javascript](javascript:alert) [safe](https://example.com)\n3. first\n   - nested\n     continuation\n| name | value |\n| --- | --- |\n| **text** | `code` |\n```sh\nprintf "<tag>"',
      );
      document.querySelector('main').append(markdown);
      return {
        images: markdown.querySelectorAll('img').length,
        links: [...markdown.querySelectorAll('a')].map((a) => [
          a.getAttribute('href'),
          a.target,
          a.rel,
        ]),
        nested: markdown.querySelector('ol[start="3"] > li > ul > li')?.textContent,
        table: markdown.querySelector('table tbody')?.textContent,
        code: markdown.querySelector('pre')?.textContent,
        text: markdown.textContent,
      };
    });
    assert.equal(result.images, 0);
    assert.deepEqual(result.links, [['https://example.com', '_blank', 'noopener noreferrer']]);
    assert.equal(result.nested, 'nestedcontinuation');
    assert.equal(result.table, 'textcode');
    assert.equal(result.code, 'printf "<tag>"');
    assert.match(result.text, /<img src=x onerror=alert\(1\)>/);
  } finally {
    await browser.close();
  }
});

test('numeric chart geometry works under the served CSP and releases its stylesheet', async () => {
  const bundle = await build({
    absWorkingDir: new URL('../', import.meta.url).pathname,
    entryPoints: ['src/lib-contract.tsx'],
    bundle: true,
    write: false,
    format: 'iife',
    globalName: 'AccountExample',
    platform: 'browser',
    tsconfig: 'tsconfig.json',
    define: { 'process.env.NODE_ENV': '"production"' },
  });
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.route('http://geometry.test/**', (route) => {
      const url = route.request().url();
      return route.fulfill({
        contentType: url.endsWith('.js')
          ? 'text/javascript'
          : url.endsWith('.css')
            ? 'text/css'
            : 'text/html',
        headers: {
          'Content-Security-Policy': "default-src 'self'; script-src 'self'; style-src 'self'",
        },
        body: url.endsWith('.js')
          ? bundle.outputFiles[0].text
          : url.endsWith('.css')
            ? '.track{width:100px}.bar{height:10px}'
            : '<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="/geometry.css"><div class="track"><div class="bar"></div></div><script src="/consumer.js"></script>',
      });
    });
    await page.goto('http://geometry.test/');
    const result = await page.evaluate(() => {
      const before = document.adoptedStyleSheets.length,
        layout = AccountExample.createMeasuredLayout(),
        bar = document.querySelector('.bar');
      bar.classList.add(layout.className('width', 37.5, '%'));
      const width = bar.getBoundingClientRect().width;
      let rejected = 0;
      for (const [property, value, unit] of [
        ['width', NaN, '%'],
        ['width', Infinity, 'px'],
        ['width', '1; background:url(https://evil.test)', '%'],
        ['background', 1, '%'],
        ['width', 1, ';color:red'],
      ]) {
        try {
          layout.className(property, value, unit);
        } catch {
          rejected++;
        }
      }
      const mounted = document.adoptedStyleSheets.length;
      layout.destroy();
      layout.destroy();
      let disposed = false;
      try {
        layout.className('width', 1, '%');
      } catch {
        disposed = true;
      }
      return {
        width,
        before,
        mounted,
        after: document.adoptedStyleSheets.length,
        rejected,
        disposed,
        inlineStyles: document.querySelectorAll('[style]').length,
      };
    });
    assert.equal(result.width, 37.5);
    assert.equal(result.mounted, result.before + 1);
    assert.equal(result.after, result.before);
    assert.equal(result.rejected, 5);
    assert.equal(result.disposed, true);
    assert.equal(result.inlineStyles, 0);
  } finally {
    await browser.close();
  }
});

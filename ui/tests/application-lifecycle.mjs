import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';

const root = new URL('../../', import.meta.url).pathname;
const binary = process.env.SEMON_BIN ?? join(root, 'target/debug/semon');
for (const width of [390, 1280])
  for (const mode of ['full', 'sidebar', 'native', 'host']) {
    test(`owned application teardown and late model response ${width} ${mode}`, async () => {
      const scratch = await mkdtemp(join(tmpdir(), 'semon-app-life-'));
      const fixture = spawnSync(process.execPath, [join(root, 'tests/ui/fixture.mjs'), scratch], {
        encoding: 'utf8',
      });
      assert.equal(fixture.status, 0, fixture.stderr);
      const server = spawn(
        binary,
        [
          'sessions',
          '--serve',
          '--listen',
          '127.0.0.1:0',
          '--claude-home',
          join(scratch, 'claude'),
          '--claude-json',
          join(scratch, '.claude.json'),
          '--codex-home',
          join(scratch, 'codex'),
          '--copilot-home',
          join(scratch, 'copilot'),
          '--proc-root',
          join(scratch, 'proc'),
          '--cache',
          join(scratch, 'index.json'),
        ],
        {
          env: { ...process.env, SEMON_TEST_NOW: fixture.stdout.trim() },
          stdio: ['ignore', 'pipe', 'pipe'],
        },
      );
      let output = '';
      for (const stream of [server.stdout, server.stderr])
        stream.on('data', (data) => {
          output += data;
        });
      const browser = await chromium.launch();
      try {
        let match;
        for (let i = 0; i < 100; i++) {
          match = output.match(/(http:\/\/127\.0\.0\.1:\d+)\/\?t=([a-f0-9]+)/);
          if (match) break;
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
        assert.ok(match, 'fixture server did not start');
        const bundle = await build({
          absWorkingDir: join(root, 'ui'),
          entryPoints: ['src/application.ts'],
          bundle: true,
          write: false,
          format: 'iife',
          globalName: 'Application',
          platform: 'browser',
          tsconfig: 'tsconfig.json',
          define: { 'process.env.NODE_ENV': '"production"' },
        });
        const page = await browser.newPage({ viewport: { width, height: 860 } }),
          errors = [];
        page.on('pageerror', (error) => errors.push(error.message));
        await page.route('**/viewer.js', (route) =>
          route.fulfill({ contentType: 'text/javascript', body: bundle.outputFiles[0].text }),
        );
        await page.goto(match[1] + '/?t=' + match[2] + '&compat=1');
        const result = await page.evaluate(async (mode) => {
          const assert = (condition, message) => {
            if (!condition) throw Error(message);
          };
          const settle = () =>
            new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          const originalFetch = window.fetch,
            model = await originalFetch('/api/model?delta=1').then((r) => r.json());
          const add = EventTarget.prototype.addEventListener,
            remove = EventTarget.prototype.removeEventListener;
          const registrations = new Map();
          const tracked = (target) =>
            target === document || target === window || target instanceof MediaQueryList;
          EventTarget.prototype.addEventListener = function (type, listener, options) {
            if (tracked(this)) {
              let list = registrations.get(this);
              if (!list) registrations.set(this, (list = []));
              if (
                !list.some(
                  (x) =>
                    x.type === type &&
                    x.listener === listener &&
                    x.capture === !!(typeof options === 'boolean' ? options : options?.capture),
                )
              )
                list.push({
                  type,
                  listener,
                  capture: !!(typeof options === 'boolean' ? options : options?.capture),
                });
            }
            return add.call(this, type, listener, options);
          };
          EventTarget.prototype.removeEventListener = function (type, listener, options) {
            if (tracked(this)) {
              const list = registrations.get(this) ?? [],
                capture = !!(typeof options === 'boolean' ? options : options?.capture);
              const index = list.findIndex(
                (x) => x.type === type && x.listener === listener && x.capture === capture,
              );
              if (index >= 0) list.splice(index, 1);
            }
            return remove.call(this, type, listener, options);
          };
          const originalTimers = {
            setTimeout: window.setTimeout,
            clearTimeout: window.clearTimeout,
            setInterval: window.setInterval,
            clearInterval: window.clearInterval,
            requestAnimationFrame: window.requestAnimationFrame,
            cancelAnimationFrame: window.cancelAnimationFrame,
          };
          const timers = new Set(),
            intervals = new Set(),
            frames = new Set();
          window.setTimeout = (callback, delay, ...args) => {
            const id = originalTimers.setTimeout.call(
              window,
              () => {
                timers.delete(id);
                callback(...args);
              },
              delay,
            );
            timers.add(id);
            return id;
          };
          window.clearTimeout = (id) => {
            timers.delete(id);
            originalTimers.clearTimeout.call(window, id);
          };
          window.setInterval = (callback, delay, ...args) => {
            const id = originalTimers.setInterval.call(window, callback, delay, ...args);
            intervals.add(id);
            return id;
          };
          window.clearInterval = (id) => {
            intervals.delete(id);
            originalTimers.clearInterval.call(window, id);
          };
          window.requestAnimationFrame = (callback) => {
            const id = originalTimers.requestAnimationFrame.call(window, (time) => {
              frames.delete(id);
              callback(time);
            });
            frames.add(id);
            return id;
          };
          window.cancelAnimationFrame = (id) => {
            frames.delete(id);
            originalTimers.cancelAnimationFrame.call(window, id);
          };
          const originalObservers = {
              MutationObserver: window.MutationObserver,
              ResizeObserver: window.ResizeObserver,
              IntersectionObserver: window.IntersectionObserver,
            },
            observers = new Map();
          for (const [name, Original] of Object.entries(originalObservers))
            window[name] = class extends Original {
              observe(target, ...args) {
                let targets = observers.get(this);
                if (!targets) observers.set(this, (targets = new Set()));
                targets.add(target);
                super.observe(target, ...args);
              }
              unobserve(target) {
                observers.get(this)?.delete(target);
                super.unobserve(target);
              }
              disconnect() {
                observers.delete(this);
                super.disconnect();
              }
            };
          const ready = async () => {
            const deadline = performance.now() + 10000;
            while (!document.querySelector('#lanes .srow')) {
              assert(performance.now() < deadline, 'boot did not draw Recent');
              await new Promise((resolve) => originalTimers.setTimeout.call(window, resolve, 25));
            }
            await settle();
          };
          const effects = () =>
            timers.size +
            intervals.size +
            frames.size +
            [...observers.values()].filter((targets) => targets.size).length;
          const count = () =>
            [...registrations.values()].reduce((sum, list) => sum + list.length, 0);
          const app = document.querySelector('.app');
          if (mode === 'sidebar') {
            app.dataset.viewer = 'sidebar';
            app.dataset.viewerNav = 'sessions';
          }
          let destroys = 0,
            accounts = 0,
            pendingResolve,
            pendingSignal,
            nativeResolve,
            nativeSignal,
            staleDestroyed = 0,
            requests = 0,
            hold = false;
          window.fetch = (path, options) => {
            if (String(path).startsWith('/api/model')) {
              requests++;
              if (hold) {
                pendingSignal = options?.signal;
                return new Promise((resolve) => {
                  pendingResolve = () =>
                    resolve(
                      new Response(JSON.stringify(model), {
                        headers: { 'Content-Type': 'application/json' },
                      }),
                    );
                });
              }
              return Promise.resolve(
                new Response(JSON.stringify(model), {
                  headers: { 'Content-Type': 'application/json' },
                }),
              );
            }
            if (String(path).startsWith('/api/tx'))
              return new Promise((resolve) =>
                originalTimers.setTimeout.call(window, resolve, 100),
              ).then(() => originalFetch(path, options));
            return originalFetch(path, options);
          };
          const host = () =>
            mode === 'native' || mode === 'host'
              ? {
                  machinesPath: '/manage/machines',
                  nativePage:
                    mode === 'native' ? { title: 'Workspace', nav: 'machines' } : undefined,
                  initialMachines: {
                    element: document.createElement('div'),
                    destroy() {
                      destroys++;
                    },
                  },
                  loadMachines(signal) {
                    nativeSignal = signal;
                    return new Promise((resolve) => {
                      nativeResolve = () =>
                        resolve({
                          element: document.createElement('div'),
                          destroy() {
                            staleDestroyed++;
                          },
                        });
                    });
                  },
                  modelAccount() {
                    accounts++;
                  },
                }
              : null;
          try {
            for (let i = 0; i < 3; i++) {
              const owner = Application.mountViewerApplication(host());
              await ready();
              assert(
                document.querySelectorAll('#lanes .srow').length > 0,
                'boot did not draw Recent',
              );
              let panelBody;
              const active = count();
              assert(active > 0, 'no document effects were owned');
              if (mode === 'full') {
                document.querySelector('#lanes .srow').click();
                for (
                  let attempt = 0;
                  attempt < 20 && !document.querySelector('#page .turns');
                  attempt++
                )
                  await new Promise((resolve) =>
                    originalTimers.setTimeout.call(window, resolve, 50),
                  );
                assert(document.querySelector('#page .turns'), 'session did not commit');
                document.querySelector('#more-btn').click();
                await settle();
                assert(document.querySelector('dialog'), 'session panel did not open');
                panelBody = document.querySelector('dialog .panel-b');
              }
              if (mode === 'host') {
                document.querySelector('#nav [data-go="home"]').click();
                await settle();
                document.querySelector('#nav [data-go="machines"]').click();
                await settle();
              }
              owner.destroy();
              owner.destroy();
              await settle();
              assert(count() === 0, 'document effects leaked after destroy: ' + count());
              assert(effects() === 0, 'timers or animation frames leaked: ' + effects());
              if (mode === 'host') {
                assert(nativeSignal.aborted, 'native destination was not aborted');
                nativeResolve();
                await settle();
                assert(staleDestroyed === i + 1, 'late native content was not destroyed');
              }
              assert(
                !document.querySelector('dialog') &&
                  !document.querySelector('#lanes .srow') &&
                  (!panelBody || !panelBody.firstChild),
                'roots survived teardown',
              );
              const before = requests;
              window.dispatchEvent(new Event('semon:refresh'));
              await settle();
              assert(requests === before, 'disposed refresh listener polled');
            }
            hold = true;
            const old = Application.mountViewerApplication(host());
            await settle();
            assert(pendingResolve, 'boot was not pending');
            old.destroy();
            assert(pendingSignal?.aborted, 'pending model was not aborted');
            const before = document.body.textContent,
              beforeAccounts = accounts;
            pendingResolve();
            await settle();
            assert(
              document.body.textContent === before && accounts === beforeAccounts,
              'late model revived a root or host callback',
            );
            hold = false;
            const first = Application.mountViewerApplication(host());
            await ready();
            const active = count();
            const second = Application.mountViewerApplication(host());
            await ready();
            assert(count() === active, 'replacement duplicated document effects');
            first.destroy();
            assert(count() === active, 'old owner destroyed replacement');
            second.destroy();
            await settle();
            assert(count() === 0 && effects() === 0, 'replacement effects leaked');
            return { requests, destroys };
          } finally {
            Object.assign(window, originalTimers, originalObservers);
            window.fetch = originalFetch;
            EventTarget.prototype.addEventListener = add;
            EventTarget.prototype.removeEventListener = remove;
          }
        }, mode);
        assert.deepEqual(errors, []);
        if (mode === 'native') assert.equal(result.destroys, 6);
      } finally {
        await browser.close();
        server.kill();
        await rm(scratch, { recursive: true, force: true });
      }
    });
  }

test('error-navigation request owners stay bounded across polls and mode changes', async () => {
  const contract = await build({
    absWorkingDir: join(root, 'ui'),
    stdin: {
      contents:
        "export { createErrorNavigation } from './src/navigation/errors'; export { EffectScope } from './src/app/effects';",
      resolveDir: join(root, 'ui'),
      loader: 'ts',
    },
    bundle: true,
    write: false,
    format: 'iife',
    globalName: 'ErrorOwnership',
    platform: 'browser',
    tsconfig: 'tsconfig.json',
  });
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.route('http://errors.test/**', (route) =>
      route.fulfill({
        contentType: route.request().url().endsWith('.js') ? 'text/javascript' : 'text/html',
        body: route.request().url().endsWith('.js')
          ? contract.outputFiles[0].text
          : '<!doctype html><main id="page"></main><script src="/contract.js"></script>',
      }),
    );
    await page.goto('http://errors.test/');
    await page.evaluate(async () => {
      const check = (ok, message) => {
        if (!ok) throw Error(message);
      };
      const retired = new ErrorOwnership.EffectScope();
      retired.destroy();
      check(retired.request().signal.aborted, 'disposed scope admitted a live request');
      const scopes = new Map(),
        prototype = ErrorOwnership.EffectScope.prototype;
      const request = prototype.request,
        release = prototype.releaseRequest,
        destroy = prototype.destroy;
      prototype.request = function () {
        const controller = request.call(this);
        let owned = scopes.get(this);
        if (!owned) scopes.set(this, (owned = new Set()));
        owned.add(controller);
        return controller;
      };
      prototype.releaseRequest = function (controller) {
        scopes.get(this)?.delete(controller);
        return release.call(this, controller);
      };
      prototype.destroy = function () {
        scopes.delete(this);
        return destroy.call(this);
      };
      const timerOwner = new ErrorOwnership.EffectScope(),
        clear0 = window.clearTimeout;
      let canceled = 0;
      window.clearTimeout = (id) => {
        canceled++;
        clear0.call(window, id);
      };
      const timer = timerOwner.timeout(() => {
        throw Error('canceled callback ran');
      }, 60000);
      timerOwner.clearTimeout(timer);
      timerOwner.destroy();
      window.clearTimeout = clear0;
      check(canceled === 1, 'a canceled timer retained its teardown cleanup');
      const retained = () => [...scopes.values()].reduce((count, items) => count + items.size, 0);
      const requestOwner = new ErrorOwnership.EffectScope(),
        aborted = requestOwner.request();
      aborted.abort();
      check(retained() === 0, 'aborted request retained scope ownership');
      requestOwner.destroy();
      let resolvePending,
        signal,
        hold = false,
        draws = 0,
        tails = 0;
      const fetch0 = window.fetch;
      window.fetch = (_url, options) =>
        hold
          ? new Promise((resolve) => {
              signal = options.signal;
              resolvePending = resolve;
            })
          : Promise.resolve(
              new Response(JSON.stringify({ slots: [], errors: 0, version: 'stable' }), {
                headers: { 'Content-Type': 'application/json' },
              }),
            );
      const host = {
        navigation: { route: { v: 'session', id: 's' } },
        TX: { s: [] },
        TXM: { s: { from: 0, to: 0, total: 0, tok: 'a' } },
        TOK: {},
        SESS: { s: { signals: {} } },
        show: { tools: true },
        sidebarOnly: false,
        page: () => document.getElementById('page'),
        drawSessionBar() {
          draws++;
        },
        render() {
          draws++;
        },
        keepFocus: (fn) => fn(),
        countOf: () => 0,
        capture: () => ({}),
        restore() {},
        opener: () => null,
        resetPagerInput() {},
        stopOpeningEndPin() {},
        clearFind() {},
        centre() {},
        fetchTx: async () => null,
        dropTx() {},
        spread() {},
        tail: async () => {
          tails++;
          return null;
        },
      };
      const owner = ErrorOwnership.createErrorNavigation(host);
      try {
        for (let mode = 0; mode < 3; mode++) {
          host.show = { tools: false };
          host.TOK = { s: 'a' };
          owner.open('s');
          check(host.show.tools, 'error mode used a stale replaced view state');
          await owner.live();
          check(retained() === 1, 'completed error-list request remained owned');
          for (let poll = 0; poll < 10; poll++) {
            await owner.live();
            check(retained() === 1, 'polls accumulated completed request controllers');
          }
          host.TXM.s = { from: 1, to: 1, total: 1 };
          owner.close();
          await Promise.resolve();
          check(!host.show.tools && tails === 0, 'error close used stale view/growth state');
          check(retained() === 0, 'closed error mode retained its request owner');
        }
        const queue = [];
        window.fetch = (_url, options) =>
          new Promise((resolve, reject) => {
            options.signal.addEventListener(
              'abort',
              () => reject(new DOMException('Aborted', 'AbortError')),
              { once: true },
            );
            queue.push({ resolve, reject });
          });
        owner.open('s');
        const refreshed = owner.live();
        queue[1].resolve(
          new Response(JSON.stringify({ slots: [], errors: 0, version: 'fresh' }), {
            headers: { 'Content-Type': 'application/json' },
          }),
        );
        await refreshed;
        await Promise.resolve();
        check(
          !owner.state.notice && !document.body.textContent.includes("Couldn't list"),
          'superseded initial request reported a false failure',
        );
        const failed = owner.live();
        queue[2].reject(new Error('Current request failed'));
        let rejected = false;
        try {
          await failed;
        } catch {
          rejected = true;
        }
        check(rejected, 'current transport failure was suppressed');
        owner.close();
        window.fetch = (_url, options) =>
          hold
            ? new Promise((resolve) => {
                signal = options.signal;
                resolvePending = resolve;
              })
            : Promise.resolve(
                new Response(JSON.stringify({ slots: [], errors: 0, version: 'stable' }), {
                  headers: { 'Content-Type': 'application/json' },
                }),
              );
        owner.open('s');
        await owner.live();
        hold = true;
        const pending = owner.live();
        check(signal && !signal.aborted, 'pending error-list request lacks a live owner');
        owner.destroy();
        check(
          signal.aborted && retained() === 0,
          'error-navigation destroy leaked a pending request',
        );
        const before = draws;
        resolvePending(
          new Response(JSON.stringify({ slots: [1], errors: 1, version: 'late' }), {
            headers: { 'Content-Type': 'application/json' },
          }),
        );
        await pending;
        check(draws === before, 'late error-list response drew after destroy');
      } finally {
        owner.destroy();
        window.fetch = fetch0;
        prototype.request = request;
        prototype.releaseRequest = release;
        prototype.destroy = destroy;
      }
    });
  } finally {
    await browser.close();
  }
});

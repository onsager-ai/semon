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
  for (const mode of ['full', 'native', 'host']) {
    test(`canonical application teardown and late overview response ${width} ${mode}`, async () => {
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
        await page.goto(match[1] + '/?t=' + match[2]);
        const result = await page.evaluate(async (mode) => {
          const assert = (condition, message) => {
            if (!condition) throw Error(message);
          };
          const settle = () =>
            new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          const originalFetch = window.fetch;
          let model;
          for (let attempt = 0; attempt < 200; attempt++) {
            model = await originalFetch('/api/viewer/home').then((r) => r.json());
            if (model.recent?.length) break;
            await new Promise((resolve) => setTimeout(resolve, 50));
          }
          assert(model.recent?.length, 'bounded producer did not publish Home');
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
          let destroys = 0,
            pendingResolve,
            pendingSignal,
            requests = 0,
            pendingObservation = false,
            hold = false;
          const requestPaths = [];
          window.fetch = (path, options) => {
            requestPaths.push(String(path).split('?')[0]);
            if (String(path).startsWith('/api/viewer/home')) {
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
                new Response(
                  JSON.stringify(
                    pendingObservation
                      ? {
                          ...model,
                          observations: [
                            { source_key: model.machines[0].key, state: 'catalog_unavailable' },
                          ],
                        }
                      : model,
                  ),
                  {
                    headers: { 'Content-Type': 'application/json' },
                  },
                ),
              );
            }
            if (String(path).startsWith('/api/session-transcript'))
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
                  navigation: {
                    leading: [
                      {
                        key: 'workspace-settings',
                        label: 'Workspace Settings',
                        icon: 'settings',
                        href: '/workspace/settings?return_to=%2Fsessions',
                        current: false,
                      },
                      {
                        key: 'new-session',
                        label: 'New Session',
                        icon: 'plus',
                        href: '/sessions/new?return_to=%2Fsessions',
                        current: false,
                      },
                    ],
                  },
                  initialMachines: {
                    element: document.createElement('div'),
                    destroy() {
                      destroys++;
                      this.element.remove();
                    },
                  },
                }
              : null;
          try {
            if (mode === 'native') {
              const nativeHost = () => {
                const value = host();
                history.replaceState(null, '', '/workspace/settings');
                document.querySelector('#page').replaceChildren(value.initialMachines.element);
                value.initialMachines.element.innerHTML =
                  '<form><input name="task" value="Retained task"></form>';
                return value;
              };
              for (let i = 0; i < 4; i++) {
                const before = requests;
                const owner = Application.mountViewerApplication(nativeHost());
                await settle();
                assert(
                  document.querySelector('#page input')?.value === 'Retained task',
                  'native content was not mounted',
                );
                assert(
                  document.querySelector('#nav [aria-current="page"]')?.dataset.go === 'machines',
                  'native active state was lost',
                );
                assert(
                  document.querySelector('#nav [data-go="machines"]')?.getAttribute('href') ===
                    '/manage/machines',
                  'native host Machines path was lost',
                );
                assert(
                  document
                    .querySelector('#nav [data-go="workspace-settings"]')
                    ?.getAttribute('href') === '/workspace/settings?return_to=%2Fsessions',
                  'shared chrome overwrote the native host return destination',
                );
                assert(
                  !document.querySelector('#nav [data-go="new-session"]'),
                  'paused creation was advertised by the native shell',
                );
                assert(
                  document.querySelector('#lanes .srow') && requests === before + 1,
                  'native chrome did not share the bounded Recent publication',
                );
                assert(
                  requestPaths.every((path) => path === '/api/viewer/home'),
                  'native chrome loaded session bodies or a workspace model',
                );
                assert(count() > 0, 'native chrome owns no document effects');
                owner.destroy();
                owner.destroy();
                await settle();
                assert(count() === 0 && effects() === 0, 'native effects leaked after teardown');
                assert(!document.querySelector('#page input'), 'native content survived teardown');
                window.dispatchEvent(new Event('semon:refresh'));
                await settle();
                assert(
                  requests === before + 1,
                  'disposed native shell started another overview read',
                );
              }
              const first = Application.mountViewerApplication(nativeHost());
              await settle();
              const active = count();
              const second = Application.mountViewerApplication(nativeHost());
              await settle();
              assert(count() === active, 'native replacement duplicated effects');
              first.destroy();
              assert(
                count() === active && document.querySelector('#page input'),
                'old native owner destroyed replacement',
              );
              second.destroy();
              await settle();
              assert(
                count() === 0 && effects() === 0 && requests === 6,
                'native replacement leaked effects or duplicated overview reads',
              );
              return { requests, destroys };
            }
            for (let i = 0; i < 3; i++) {
              history.replaceState(null, '', '/');
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
                assert(
                  !document.querySelector('[aria-label="Open Composer"]'),
                  'paused Composer was advertised',
                );
                const sessionMenu = document.querySelector('#more-btn[aria-label="Session menu"]');
                assert(sessionMenu, 'selected session menu trigger is missing');
                sessionMenu.click();
                await settle();
                assert(document.querySelector('dialog'), 'session menu panel did not open');
                panelBody = document.querySelector('dialog .panel-b');
              }
              if (mode === 'host') {
                document.querySelector('#nav [data-go="home"]').click();
                await settle();
                assert(
                  document.querySelector('#nav [data-go="machines"]').getAttribute('href') ===
                    '/manage/machines',
                  'native destination path changed',
                );
              }
              owner.destroy();
              owner.destroy();
              await settle();
              assert(count() === 0, 'document effects leaked after destroy: ' + count());
              assert(effects() === 0, 'timers or animation frames leaked: ' + effects());

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
            history.replaceState(null, '', '/');
            pendingObservation = true;
            const retrying = Application.mountViewerApplication(host());
            await ready();
            assert(
              document.body.textContent.includes('Retrying'),
              'pending observation was not shown',
            );
            retrying.destroy();
            await settle();
            assert(count() === 0 && effects() === 0, 'pending overview retry survived teardown');
            const retiredRequests = requests;
            await new Promise((resolve) => originalTimers.setTimeout.call(window, resolve, 600));
            assert(requests === retiredRequests, 'retired overview retry made another request');
            pendingObservation = false;
            history.replaceState(null, '', '/');
            hold = true;
            const old = Application.mountViewerApplication(host());
            await settle();
            assert(pendingResolve, 'boot was not pending');
            old.destroy();
            assert(pendingSignal?.aborted, 'pending model was not aborted');
            const before = document.body.textContent;
            pendingResolve();
            await settle();
            assert(document.body.textContent === before, 'late overview revived a root');
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

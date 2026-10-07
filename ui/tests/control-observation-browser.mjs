import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const bundle = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents:
      "export {createControlObservation} from './src/app/controlObservation';export {createLocalControl} from './src/app/localControl';export {EffectScope} from './src/app/effects';export {ViewUpdates} from './src/state/viewUpdates';export {renderSessionScreen,updateSessionControl} from './src/lib/transcript';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'Controls',
  platform: 'browser',
});
for (const width of [390, 1280])
  test(`bounded control observation preserves inputs and rejects stale selection at ${width}px`, async () => {
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({ viewport: { width, height: 900 } });
      await page.route('http://controls.test/**', (route) =>
        route.fulfill({ contentType: 'text/html', body: '<!doctype html><div id="page"></div>' }),
      );
      await page.goto('http://controls.test/');
      await page.clock.install({ time: new Date('2026-10-07T00:00:00Z') });
      await page.clock.pauseAt(new Date('2026-10-07T00:01:00Z'));
      await page.addScriptTag({ content: bundle.outputFiles[0].text });
      await page.evaluate(() => {
        window.sources = [];
        window.EventSource = class extends EventTarget {
          constructor(path) {
            super();
            this.path = path;
            this.closed = false;
            sources.push(this);
          }
          close() {
            this.closed = true;
          }
          emit(kind, value) {
            this.dispatchEvent(
              new MessageEvent(kind, {
                data: typeof value === 'string' ? value : JSON.stringify(value),
              }),
            );
          }
        };
        window.native = (thread) => ({
          thread,
          generation: 'g',
          activeTurn: null,
          connected: true,
          reason: null,
          capabilities: {
            input: true,
            steer: true,
            interrupt: false,
            commandApproval: false,
            fileApproval: false,
            questions: false,
          },
          requests: [],
          actions: {},
        });
        window.route = { v: 'session', id: 'one' };
        const scope = (window.effect = new Controls.EffectScope());
        window.refreshes = 0;
        const owner = (window.owner = Controls.createLocalControl(scope, () => {
          refreshes++;
          Controls.updateSessionControl(document.querySelector('#page'), owner.view(route.id));
        }));
        const updates = new Controls.ViewUpdates();
        window.host = {
          scope,
          controlOwner: owner,
          updates,
          navigation: {
            get route() {
              return window.route;
            },
            subscribeRoute(fn) {
              window.changed = fn;
              return () => (window.changed = null);
            },
          },
          modelStore: { sessions: { one: { machine: 'a' }, two: { machine: 'b' } } },
          viewerHost: { controlStream: '/status' },
        };
        window.observation = Controls.createControlObservation(host);
        const snapshot = {
          id: 'one',
          name: 'Retained conversation',
          blocks: [],
          order: [],
          empty: 'History retained',
        };
        Controls.renderSessionScreen(document.querySelector('#page'), snapshot, { committed() {} });
        // A control repaint must never traverse the cached transcript blocks.
        snapshot.blocks.map = () => {
          throw Error('status traversed transcript blocks');
        };

        sources[0].emit('control', {
          selected: 'one',
          machine: 'a',
          revision: 'r1',
          control: native('one'),
        });
      });
      await page.locator('textarea').fill('retained draft');
      await page.locator('textarea').focus();
      await page.evaluate(() => {
        window.input = document.querySelector('textarea');
        sources[0].emit('control', {
          selected: 'one',
          machine: 'a',
          revision: 'r2',
          control: { ...native('one'), connected: false, reason: 'Provider observation stale' },
        });
      });
      assert.deepEqual(
        await page.evaluate(() => ({
          same: input === document.querySelector('textarea'),
          focus: input === document.activeElement,
          draft: input.value,
          history: document.querySelector('#page').textContent.includes('History retained'),
        })),
        { same: true, focus: true, draft: 'retained draft', history: true },
      );
      await page.evaluate(() =>
        sources[0].emit('control', {
          selected: 'one',
          machine: 'a',
          revision: 'final',
          control: {
            ...native('one'),
            connected: false,
            generation: '',
            reason: 'Environment shutdown was confirmed.',
            runtime: { state: 'ended', phase: 'ended', freshness: 'current', reconnectable: false },
            capabilities: Object.fromEntries(
              Object.keys(native('one').capabilities).map((key) => [key, false]),
            ),
          },
        }),
      );
      assert.deepEqual(
        await page.evaluate(() => ({
          same: input === document.querySelector('textarea'),
          focus: input === document.activeElement,
          draft: input.value,
          ended: document.querySelector('#page').textContent.includes('This session has ended.'),
          reconnect: [...document.querySelectorAll('button')].some(
            (button) => button.textContent.trim() === 'Reconnect',
          ),
        })),
        { same: true, focus: true, draft: 'retained draft', ended: true, reconnect: false },
      );
      await page.evaluate(() => sources[0].emit('error', 'temporary'));
      assert.equal(await page.evaluate(() => sources[0].closed), true);
      await page.clock.runFor(1001);
      assert.equal(await page.evaluate(() => sources.length), 2);
      await page.evaluate(() => {
        sources[1].emit('control', {
          selected: 'one',
          machine: 'a',
          revision: 'r3',
          control: native('one'),
        });
        window.route = { v: 'session', id: 'two' };
        changed();
        sources[1].emit('control', {
          selected: 'one',
          machine: 'a',
          revision: 'late',
          control: native('one'),
        });
        sources[1].emit('ended', 'late');
      });
      assert.equal(
        await page.evaluate(() => sources.at(-1).path),
        '/status?selected=two&machine=b',
      );
      assert.equal(await page.evaluate(() => owner.view('one')), undefined);
      await page.evaluate(() =>
        sources.at(-1).emit('control', {
          selected: 'two',
          machine: 'b',
          revision: 'two-r1',
          control: native('two'),
        }),
      );
      assert.equal(await page.evaluate(() => owner.view('two').snapshot.thread), 'two');
      // A quiet stalled stream reaches an owned retry; heartbeats extend only its own deadline.
      await page.clock.runFor(29000);
      await page.evaluate(() => sources.at(-1).emit('heartbeat', 'current'));
      await page.clock.runFor(2000);
      assert.equal(await page.evaluate(() => sources.length), 3);
      await page.clock.runFor(29001);
      assert.equal(await page.evaluate(() => sources.length), 4);
      await page.evaluate(() => {
        observation.destroy();
        effect.destroy();
        sources.at(-1).emit('control', {
          selected: 'two',
          machine: 'b',
          revision: 'after',
          control: native('two'),
        });
      });
      await page.clock.runFor(60000);
      assert.equal(await page.evaluate(() => sources.length), 4);
      assert.equal(await page.evaluate(() => changed), null);
    } finally {
      await browser.close();
    }
  });

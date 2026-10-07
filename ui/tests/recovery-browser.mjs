import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const bundle = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents:
      "export {createBootstrap} from './src/app/bootstrap';export {createLiveModel} from './src/app/liveModel';export {EffectScope} from './src/app/effects';export {mountNativeShell} from './src/app/native-shell';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'Recovery',
  platform: 'browser',
});
async function fixture(run) {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.route('http://recovery.test/**', (route) =>
      route.fulfill({
        contentType: 'text/html',
        body: '<!doctype html><div id="page"></div><div id="lanes"></div><textarea>retained draft</textarea><span data-poll="/ready" data-poll-go="/complete">Waiting</span>',
      }),
    );
    await page.goto('http://recovery.test/s/codex/s?turn=t#anchor');
    await page.clock.install({ time: new Date('2026-10-07T00:00:00Z') });
    await page.clock.pauseAt(new Date('2026-10-07T00:01:00Z'));
    await page.addScriptTag({ content: bundle.outputFiles[0].text });
    await run(page);
  } finally {
    await browser.close();
  }
}
for (const embedded of [false, true])
  test(`first model and deep transcript failures recover, embedded=${embedded}`, async () =>
    fixture(async (page) => {
      await page.evaluate((embedded) => {
        const scope = new Recovery.EffectScope();
        window.bootState = { reads: 0, loads: 0, renders: 0, reveals: 0, schedules: 0 };
        const host = (window.bootHost = {
          disposed: false,
          sidebarOnly: false,
          viewerHost: embedded ? { modelFailed: () => false } : null,
          navigation: { route: { v: 'home' } },
          scope,
          modelStore: {
            sessions: { s: { harness: 'codex' } },
            machines: {},
            turn: new Map([['t', { id: 't', sid: 's' }]]),
          },
          transportOwner: {
            api: async () => {
              if (++bootState.reads === 1) throw Object.assign(Error('temporary'), { status: 503 });
              return { version: 'v1' };
            },
            adopt: (x) => x,
            enc: encodeURIComponent,
            load: async () => {
              if (++bootState.loads === 1)
                throw Object.assign(Error('transcript temporary'), { status: 503 });
            },
          },
          liveModelOwner: {
            LIVE: {},
            remember() {},
            schedule() {
              bootState.schedules++;
            },
          },
          documentRendererOwner: {
            render() {
              bootState.renders++;
            },
          },
          destination: {
            revealTurn() {
              bootState.reveals++;
            },
            revealEntryHash() {},
            openSessionAtEnd() {},
          },
          viewport: { syncJump() {} },
          historyScrollOwner: { quietTop() {} },
          tickerOwner: { ticker() {} },
          $: (s) => document.querySelector(s),
        });
        window.bootScope = scope;
        Recovery.createBootstrap(host).boot();
      }, embedded);
      await page.clock.runFor(1100);
      await page.clock.runFor(2100);
      assert.deepEqual(await page.evaluate(() => ({ ...bootState })), {
        reads: 3,
        loads: 2,
        renders: 1,
        reveals: 1,
        schedules: 1,
      });
      assert.equal(await page.evaluate(() => bootHost.navigation.route.turn), 't');
      assert.equal(await page.locator('textarea').inputValue(), 'retained draft');
      await page.evaluate(() => {
        bootHost.disposed = true;
        bootScope.destroy();
      });
      await page.clock.runFor(31000);
      assert.equal(await page.evaluate(() => bootState.reads), 3);
    }));

test('stream application recovery uses snapshots and retains status until synchronization', async () =>
  fixture(async (page) => {
    await page.evaluate(() => {
      window.sources = [];
      window.EventSource = class extends EventTarget {
        closed = false;
        constructor() {
          super();
          sources.push(this);
        }
        close() {
          this.closed = true;
        }
      };
      window.applied = [];
      window.reads = 0;
      window.failUpdate = true;
      const host = (window.liveHost = {
        disposed: false,
        viewerHost: { modelStream: '/events' },
        $: (s) => document.querySelector(s),
        transportOwner: {
          enc: encodeURIComponent,
          api: async (path) => {
            reads++;
            if (path !== '/api/model?delta=1&since=') throw Error('not a snapshot');
            if (reads === 1) throw Object.assign(Error('temporary'), { status: 503 });
            return { version: 'snapshot' };
          },
        },
        liveUpdates: {
          applyModelDelta: (x) => x,
          update: async (x) => {
            if (failUpdate) {
              failUpdate = false;
              throw Error('required transcript unavailable');
            }
            applied.push(x.version);
          },
        },
      });
      window.live = Recovery.createLiveModel(host);
      live.LIVE.version = 'v1';
      live.schedule(0);
      sources[0].dispatchEvent(new MessageEvent('model', { data: '{"version":"failed"}' }));
    });
    await page.waitForSelector('.livenote');
    await page.evaluate(() => sources[0].dispatchEvent(new Event('open')));
    assert.equal(await page.locator('.livenote').count(), 1);
    await page.clock.runFor(1100);
    assert.equal(await page.locator('.livenote').count(), 1);
    await page.clock.runFor(4100);
    assert.deepEqual(await page.evaluate(() => applied), ['snapshot']);
    assert.equal(await page.locator('.livenote').count(), 0);
    assert.equal(await page.locator('textarea').inputValue(), 'retained draft');
    await page.evaluate(() => {
      sources[0].dispatchEvent(new MessageEvent('model', { data: 'malformed' }));
    });
    await page.waitForSelector('.livenote');
    await page.evaluate(() => live.destroy());
    await page.clock.runFor(31000);
    assert.equal(await page.evaluate(() => reads), 2);
    assert.equal(await page.evaluate(() => sources[0].closed), true);
  }));

test('native hanging polls timeout, retry, and redirect once', async () =>
  fixture(async (page) => {
    await page.evaluate(() => {
      window.requests = [];
      window.fetch = (_url, options) =>
        new Promise((resolve) => requests.push({ signal: options.signal, resolve }));
      window.native = Recovery.mountNativeShell({ chrome: false });
    });
    await page.clock.runFor(3000);
    assert.equal(await page.evaluate(() => requests.length), 1);
    await page.clock.runFor(10000);
    assert.equal(await page.evaluate(() => requests[0].signal.aborted), true);
    assert.match(await page.locator('[role=status]').innerText(), /temporarily unavailable/);
    await page.clock.runFor(2000);
    assert.equal(await page.evaluate(() => requests.length), 2);
    await page.evaluate(() => requests[0].resolve({ status: 200 }));
    assert.match(page.url(), /\/s\/codex\/s/);
    await page.evaluate(() => requests[1].resolve({ status: 200 }));
    await page.waitForURL('http://recovery.test/complete');
  }));
for (const mode of ['remove', 'expire', 'remount'])
  test(`native poll ${mode} cancels active request and ignores late success`, async () =>
    fixture(async (page) => {
      await page.evaluate(() => {
        window.requests = [];
        window.fetch = (_url, options) =>
          new Promise((resolve) => requests.push({ signal: options.signal, resolve }));
        window.native = Recovery.mountNativeShell({ chrome: false });
      });
      await page.clock.runFor(3000);
      if (mode === 'expire') await page.clock.fastForward(30 * 60 * 1000);
      else
        await page.evaluate((mode) => {
          if (mode === 'remove') document.querySelector('[data-poll]').remove();
          else window.replacement = Recovery.mountNativeShell({ chrome: false });
        }, mode);
      assert.equal(await page.evaluate(() => requests[0].signal.aborted), true);
      await page.evaluate(() => requests[0].resolve({ status: 200 }));
      assert.match(page.url(), /\/s\/codex\/s/);
      if (mode === 'expire')
        assert.match(await page.locator('[role=status]').innerText(), /expired/);
      await page.evaluate(() => {
        native.destroy();
        window.replacement?.destroy();
      });
      const count = await page.evaluate(() => requests.length);
      await page.clock.runFor(31000);
      assert.equal(await page.evaluate(() => requests.length), count);
    }));

test('initial retry teardown and host-handled termination stop subsequent reads', async () =>
  fixture(async (page) => {
    for (const handled of [false, true]) {
      await page.evaluate((handled) => {
        window.initialReads = 0;
        const scope = (window.initialScope = new Recovery.EffectScope());
        const host = (window.initialHost = {
          disposed: false,
          sidebarOnly: true,
          viewerHost: { modelFailed: () => handled },
          scope,
          navigation: { route: { v: 'home' } },
          modelStore: { sessions: {}, machines: {}, turn: new Map() },
          transportOwner: {
            api: async () => {
              initialReads++;
              throw Object.assign(Error('temporary'), { status: 503 });
            },
          },
          $: (s) => document.querySelector(s),
        });
        Recovery.createBootstrap(host).boot();
      }, handled);
      if (!handled)
        await page.evaluate(() => {
          initialHost.disposed = true;
          initialScope.destroy();
        });
      await page.clock.runFor(31000);
      assert.equal(await page.evaluate(() => initialReads), 1);
      await page.evaluate(() => initialScope.destroy());
    }
  }));

test('stream events received during snapshot recovery cannot overwrite the resynchronized model', async () =>
  fixture(async (page) => {
    await page.evaluate(() => {
      window.sources = [];
      window.requests = [];
      window.applied = [];
      window.EventSource = class extends EventTarget {
        constructor() {
          super();
          sources.push(this);
        }
        close() {}
      };
      window.live = Recovery.createLiveModel({
        disposed: false,
        viewerHost: { modelStream: '/events' },
        $: (s) => document.querySelector(s),
        transportOwner: {
          enc: encodeURIComponent,
          api: () => new Promise((resolve) => requests.push(resolve)),
        },
        liveUpdates: { applyModelDelta: (x) => x, update: async (x) => applied.push(x.version) },
      });
      live.LIVE.version = 'initial';
      live.schedule(0);
      sources[0].dispatchEvent(new Event('unavailable'));
    });
    await page.clock.runFor(1);
    await page.evaluate(() => {
      sources[0].dispatchEvent(new MessageEvent('model', { data: '{"version":"stale"}' }));
      requests[0]({ version: 'snapshot-one' });
    });
    assert.equal(await page.locator('.livenote').count(), 1);
    await page.clock.runFor(4100);
    await page.evaluate(() => requests[1]({ version: 'snapshot-two' }));
    assert.deepEqual(await page.evaluate(() => applied), ['snapshot-one', 'snapshot-two']);
    assert.equal(await page.locator('.livenote').count(), 0);
    await page.evaluate(() => live.destroy());
  }));

test('selected hosted streams bind both runtimes and reject callbacks after navigation', async () =>
  fixture(async (page) => {
    await page.evaluate(() => {
      window.sources = [];
      window.EventSource = class extends EventTarget {
        constructor(url) {
          super();
          this.url = url;
          this.closed = false;
          sources.push(this);
        }
        close() {
          this.closed = true;
        }
      };
      window.applied = [];
      window.readPaths = [];
      let route = { v: 'session', id: 'a' };
      const listeners = new Set();
      window.nav = {
        get route() {
          return route;
        },
        set route(value) {
          route = value;
          for (const fn of listeners) fn();
        },
        subscribeRoute(fn) {
          listeners.add(fn);
          return () => listeners.delete(fn);
        },
      };
      window.selectedSessions = { a: { machine: 'machine-a' }, b: { machine: 'machine-b' } };
      window.live = Recovery.createLiveModel({
        navigation: nav,
        disposed: false,
        viewerHost: { modelStream: '/events', selectedModel: true },
        modelStore: { sessions: selectedSessions },
        $: (s) => document.querySelector(s),
        transportOwner: {
          enc: encodeURIComponent,
          api: async (path) => {
            readPaths.push(path);
            return { version: 'recovered-b' };
          },
        },
        liveUpdates: {
          applyModelDelta: (x) => x,
          update: async (x) => {
            if (x.newMachine) selectedSessions.b.machine = x.newMachine;
            applied.push(x.version);
          },
        },
      });
      live.LIVE.version = 'v1';
      live.schedule(0);
      sources[0].dispatchEvent(new MessageEvent('model', { data: '{"version":"old-queued"}' }));
      nav.route = { v: 'session', id: 'b' };
      sources[0].dispatchEvent(new MessageEvent('model', { data: '{"version":"old-late"}' }));
      sources[0].dispatchEvent(new Event('ended'));
      sources[1].dispatchEvent(new MessageEvent('model', { data: '{"version":"selected-b"}' }));
    });
    assert.deepEqual(await page.evaluate(() => sources.map((s) => s.url)), [
      '/events?selected=a&machine=machine-a',
      '/events?selected=b&machine=machine-b',
    ]);
    assert.equal(await page.evaluate(() => sources[0].closed), true);
    assert.deepEqual(await page.evaluate(() => applied), ['selected-b']);
    await page.evaluate(() => sources[1].dispatchEvent(new Event('unavailable')));
    await page.clock.runFor(1100);
    assert.deepEqual(await page.evaluate(() => readPaths), [
      '/api/model?delta=1&since=&selected=b&machine=machine-b',
    ]);
    assert.deepEqual(await page.evaluate(() => applied), ['selected-b', 'recovered-b']);
    await page.evaluate(() =>
      sources[1].dispatchEvent(
        new MessageEvent('model', {
          data: '{"version":"projection-changed","newMachine":"renamed-b"}',
        }),
      ),
    );
    assert.equal(await page.evaluate(() => sources.length), 3);
    assert.equal(await page.evaluate(() => sources[2].url), '/events?selected=b&machine=renamed-b');
    await page.evaluate(() =>
      sources[1].dispatchEvent(
        new MessageEvent('model', {
          data: '{"version":"late-projection"}',
        }),
      ),
    );
    await page.evaluate(() => {
      live.destroy();
      nav.route = { v: 'session', id: 'a' };
      sources[1].dispatchEvent(new MessageEvent('model', { data: '{"version":"destroyed"}' }));
    });
    assert.equal(await page.evaluate(() => sources.length), 3);
    assert.deepEqual(await page.evaluate(() => applied), [
      'selected-b',
      'recovered-b',
      'projection-changed',
    ]);
  }));

test('explicit compatibility reader remains selected across navigation and reload URLs', async () =>
  fixture(async (page) => {
    const urls = await page.evaluate(() => {
      history.replaceState(null, '', '/sessions?compat=1');
      const owner = Recovery.createBootstrap({
        modelStore: {
          sessions: { s: { harness: 'codex' } },
          machines: {},
          turn: new Map(),
        },
      });
      return [owner.urlOf({ v: 'session', id: 's', turn: 't' }), owner.urlOf({ v: 'home' })];
    });
    assert.equal(new URL(urls[0], 'http://recovery.test').searchParams.get('compat'), '1');
    assert.equal(new URL(urls[0], 'http://recovery.test').searchParams.get('turn'), 't');
    assert.equal(new URL(urls[1], 'http://recovery.test').searchParams.get('compat'), '1');
  }));

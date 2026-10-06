import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: ['src/app/liveModel.ts'],
  absWorkingDir: new URL('..', import.meta.url).pathname,
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { createLiveModel } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('host stream replaces polling, serializes models and closes on teardown', async () => {
  const old = {
    window: globalThis.window,
    document: globalThis.document,
    EventSource: globalThis.EventSource,
  };
  const sources = [];
  globalThis.window = Object.assign(new EventTarget(), { setTimeout, clearTimeout });
  globalThis.document = Object.assign(new EventTarget(), { visibilityState: 'visible' });
  globalThis.EventSource = class extends EventTarget {
    closed = false;
    constructor(url) {
      super();
      this.url = url;
      sources.push(this);
    }
    close() {
      this.closed = true;
    }
  };
  let polls = 0;
  const models = [];
  const host = {
    disposed: false,
    viewerHost: { modelStream: '/api/model/events' },
    transportOwner: {
      api: async () => {
        polls++;
      },
      enc: encodeURIComponent,
    },
    liveUpdates: {
      update: async (value) => {
        models.push(value);
      },
    },
    $: () => null,
  };
  const owner = createLiveModel(host);
  try {
    owner.LIVE.version = 'v1';
    owner.schedule(0);
    owner.schedule(0);
    assert.equal(sources.length, 1);
    assert.equal(sources[0].url, '/api/model/events');
    globalThis.window.dispatchEvent(new Event('semon:refresh'));
    assert.equal(owner.LIVE.timer, null);
    sources[0].dispatchEvent(
      new MessageEvent('model', { data: JSON.stringify({ version: 'v2' }) }),
    );
    sources[0].dispatchEvent(
      new MessageEvent('model', { data: JSON.stringify({ version: 'v3' }) }),
    );
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(models, [{ version: 'v2' }, { version: 'v3' }]);
    assert.equal(polls, 0);
    owner.destroy();
    assert.equal(sources[0].closed, true);
    sources[0].dispatchEvent(new MessageEvent('model', { data: '{}' }));
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(models.length, 2);
  } finally {
    owner.destroy();
    Object.assign(globalThis, old);
  }
});

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: ['src/app/controlObservation.ts'],
  absWorkingDir: new URL('..', import.meta.url).pathname,
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { createControlObservation } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('catalog status uses accepted exact scope without legacy model access or alias guessing', () => {
  const old = globalThis.EventSource;
  const streams = [];
  globalThis.EventSource = class extends EventTarget {
    constructor(path) {
      super();
      this.path = path;
      streams.push(this);
    }
    close() {
      this.closed = true;
    }
    emit(value) {
      this.dispatchEvent(new MessageEvent('control', { data: JSON.stringify(value) }));
    }
  };
  let selected = {
    source_key: 'source',
    catalog_key: 'canonical',
    harness: 'codex',
    native_id: 'native',
  };
  let route = { v: 'session', id: 'canonical' };
  let selectionChanged,
    routeChanged,
    adopted,
    unavailable = 0;
  const timers = new Map();
  const host = {
    scope: {
      timeout(fn) {
        const id = timers.size + 1;
        timers.set(id, fn);
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
    },
    navigation: {
      get route() {
        return route;
      },
      subscribeRoute(fn) {
        routeChanged = fn;
        return () => {
          routeChanged = null;
        };
      },
    },
    modelStore: {
      get sessions() {
        throw Error('catalog status loaded legacy sessions');
      },
    },
    updates: {
      subscribe() {
        return () => {};
      },
    },
    catalogSelection: {
      selectedIdentity: () => selected,
      subscribe(fn) {
        selectionChanged = fn;
        return () => {
          selectionChanged = null;
        };
      },
    },
    viewerHost: { controlStream: '/legacy', catalogControlStream: '/catalog/status' },
    controlOwner: {
      prepare: (value) => value,
      observe(value) {
        adopted = value;
      },
      unavailable() {
        unavailable++;
      },
    },
  };
  let observation;
  try {
    observation = createControlObservation(host);
    assert.equal(
      streams[0].path,
      '/catalog/status?source_key=source&catalog_key=canonical&native_id=native&harness=codex',
    );
    const value = { ...selected, revision: 'one', control: { thread: 'native' } };
    streams[0].emit(value);
    assert.equal(adopted.thread, 'native');
    streams[0].emit({ ...value, source_key: 'foreign', revision: 'wrong' });
    assert.equal(unavailable, 1);
    assert.equal(streams[0].closed, true);
    selected = { ...selected, source_key: 'other-source', catalog_key: 'other-key' };
    route = { v: 'session', id: 'other-key' };
    selectionChanged();
    assert.equal(adopted, null);
    assert.equal(streams.length, 2);
    streams[0].emit(value);
    assert.equal(adopted, null);
    streams[1].emit({ ...selected, revision: 'two', control: { thread: 'native' } });
    assert.equal(adopted.thread, 'native');
    selected = { ...selected, native_id: null };
    selectionChanged();
    assert.equal(adopted, null);
    assert.equal(streams[1].closed, true);
    assert.equal(streams.length, 2, 'ambiguous native identity opened legacy fallback');
    selected = null;
    selectionChanged();
    assert.equal(streams.length, 2);
  } finally {
    observation?.destroy();
    globalThis.EventSource = old;
  }
  assert.equal(selectionChanged, null);
  assert.equal(routeChanged, null);
  assert.equal(timers.size, 0);
});

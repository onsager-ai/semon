import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: ['src/app/runtimeObservation.ts'],
  absWorkingDir: new URL('..', import.meta.url).pathname,
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { createRuntimeObservation } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('readonly phase starts before native identity and fences navigation, failures and teardown', () => {
  const oldSource = globalThis.EventSource,
    oldDocument = globalThis.document;
  const streams = [],
    timers = new Map();
  let seq = 0,
    selectionChanged,
    routeChanged;
  globalThis.document = new EventTarget();
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
      this.dispatchEvent(new MessageEvent('runtime', { data: JSON.stringify(value) }));
    }
  };
  let selected = { source_key: 'source', catalog_key: 'canonical' },
    route = { v: 'session', id: 'canonical' };
  const refresh = () => document.dispatchEvent(new Event('semon:refresh'));
  const host = {
    scope: {
      timeout(fn, delay) {
        const id = ++seq;
        timers.set(id, { fn, delay });
        return id;
      },
      clearTimeout(id) {
        timers.delete(id);
      },
      listen(target, event, fn) {
        target.addEventListener(event, fn);
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
    viewerHost: { catalogRuntimeStream: '/runtime/events' },
    catalogRuntimeSelection: {
      selectedScope: () => selected,
      subscribe(fn) {
        selectionChanged = fn;
        return () => {
          selectionChanged = null;
        };
      },
    },
    get modelStore() {
      throw Error('global model read');
    },
    get controlOwner() {
      throw Error('native control authority read');
    },
  };
  let owner;
  const envelope = (scope, state = 'ended', revision = 'a'.repeat(64)) => ({
    ...scope,
    revision,
    runtime: {
      state,
      phase: state,
      freshness: 'stale',
      presence: 'absent',
      caps: { send: true },
      reconnectable: true,
    },
    reason: null,
  });
  try {
    owner = createRuntimeObservation(host);
    assert.equal(streams[0].path, '/runtime/events?source_key=source&catalog_key=canonical');
    assert.equal(owner.view().delivery, 'updating');
    streams[0].emit(envelope(selected));
    assert.equal(owner.view().observation.state, 'ended');
    assert.equal(owner.view().delivery, 'current');
    assert.equal(
      owner.view().observation.freshness,
      'stale',
      'delivery cannot invent durable freshness',
    );
    assert.equal(owner.view().observation.caps, undefined);
    assert.equal(owner.view().observation.reconnectable, undefined);
    selectionChanged();
    assert.equal(streams.length, 1, 'same scope health check restarted phase stream');
    const first = streams[0];
    selected = { source_key: 'other', catalog_key: 'other' };
    route = { v: 'session', id: 'other' };
    selectionChanged();
    selected = { source_key: 'source', catalog_key: 'canonical' };
    route = { v: 'session', id: 'canonical' };
    selectionChanged();
    first.emit(envelope(selected, 'failed'));
    assert.equal(owner.view().observation, null, 'old A reply applied after A→B→A');
    streams[2].emit(envelope(selected, 'disconnected', 'b'.repeat(64)));
    streams[2].dispatchEvent(new Event('error'));
    assert.equal(owner.view().delivery, 'stale');
    assert.equal(owner.view().observation.state, 'disconnected');
    assert.equal(streams[2].closed, true);
    const retry = [...timers.entries()].find(([, v]) => v.delay === 1000);
    assert.ok(retry);
    timers.delete(retry[0]);
    retry[1].fn();
    assert.equal(streams.length, 4);
    streams[3].emit(envelope({ ...selected, source_key: 'foreign' }));
    assert.equal(streams[3].closed, true);
    refresh();
    assert.equal(streams.length, 5);
    streams[4].dispatchEvent(new Event('ended'));
    assert.equal(owner.view().delivery, 'unavailable');
    assert.equal(timers.size, 0);
    refresh();
    assert.equal(streams.length, 6);
    route = { v: 'home' };
    routeChanged();
    assert.equal(owner.view(), null);
    assert.equal(streams[5].closed, true);
  } finally {
    owner?.destroy();
    globalThis.EventSource = oldSource;
    globalThis.document = oldDocument;
  }
  assert.equal(selectionChanged, null);
  assert.equal(routeChanged, null);
  assert.equal(timers.size, 0);
});

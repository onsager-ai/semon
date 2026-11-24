import assert from 'node:assert/strict';
import { test } from 'node:test';
import { build } from 'esbuild';

const { outputFiles } = await build({
  stdin: {
    contents: `
      export { requestJson } from './src/lib/api.ts';
      export { CatalogRecovery } from './src/state/catalog-recovery.ts';
      export { CatalogNativeTranscriptStore, readNativeTranscript } from './src/state/catalog-transcript.ts';
    `,
    resolveDir: new URL('..', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { requestJson, CatalogRecovery, CatalogNativeTranscriptStore, readNativeTranscript } =
  await import(
    'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
  );

const selection = { source_key: 'local', catalog_key: 'example' };
const binding = 'a'.repeat(64);
const source = {
  root: 'claude',
  path: 'example.jsonl',
  native_id: 'example',
  offset: 42,
  prefix_sha256: Array(32).fill(1),
  tail_sha256: Array(32).fill(1),
};
const page = {
  ...selection,
  api: 1,
  projection: 7,
  read_scope: 'current',
  binding,
  errors_generation: binding,
  position: 'latest',
  earlier_cursor: null,
  identity: {
    ...selection,
    read_scope: 'current',
    harness: 'claude',
    native_id: 'example',
    native_ids: ['example'],
    source_refs: [{ source, state: 'cached' }],
    generation: binding,
    machine_label: null,
    observed_at: null,
    freshness: { state: 'cached' },
  },
  selected: { harness: 'claude', native_id: 'example', title: 'Example', model: null },
  session: null,
  entries: [
    {
      k: 'u',
      text: 'Recovered transcript entry',
      entry_id: 'prompt',
      provenance: { source, native_event_id: null, offset: 1, block: 0 },
      address: {
        record: { path: source.path, offset: 1, block: 0, occurrence: 0 },
        binding,
        block: 0,
        entry_id: 'prompt',
      },
    },
  ],
  relationship_context: { state: 'complete', turn_ids: [], handoffs: [] },
};

function fixture(t, answers, scope = 'current') {
  const paths = [];
  t.mock.method(globalThis, 'fetch', async (path) => {
    const answer = answers[paths.push(path) - 1];
    const [status, body] = typeof answer === 'function' ? await answer() : answer;
    return Response.json(body, { status });
  });
  const store = new CatalogNativeTranscriptStore(selection, scope);
  const delays = [],
    timeouts = [];
  const recovery = new CatalogRecovery();
  const read = (window = { position: 'latest' }) =>
    readNativeTranscript(store, window, {
      read(timeoutMs) {
        timeouts.push(timeoutMs);
        const query = new URLSearchParams({ sid: 'example', scope });
        if (window.position === 'earlier') query.set('cursor', window.cursor);
        return requestJson('/api/session-transcript?' + query);
      },
      current: () => true,
      recovery,
      retry: (delay) => delays.push(delay),
    });
  return { store, read, delays, timeouts, recovery, paths, requests: () => paths.length };
}

const failure = (status, code, retryable, progress) => [
  status,
  { error: { code, message: code, retryable, progress } },
];

test('the initial native transcript loads after history, Catalog and retained retries', async (t) => {
  t.mock.method(Math, 'random', () => 0.5);
  const { store, read, delays, requests } = fixture(t, [
    failure(503, 'history_rebuilding', true, { done: 1, total: 4 }),
    failure(503, 'catalog_unavailable', true),
    failure(503, 'retained_source_preparing', true),
    [200, page],
  ]);
  const backoff = [500, 1000, 2000];
  for (const [attempt, delay] of backoff.entries()) {
    assert.equal(await read(), false);
    assert.equal(delays.at(-1), delay);
    assert.equal(delays.length, attempt + 1);
    assert.equal(store.selectedPage(), null);
  }
  assert.equal(await read(), true);
  assert.equal(requests(), 4);
  assert.deepEqual(delays, backoff);
  assert.equal(store.entries()[0].entry.text, 'Recovered transcript entry');
});

test('the initial native transcript surfaces a non-retryable 503 without scheduling', async (t) => {
  const { store, read, delays, requests } = fixture(t, [
    failure(503, 'capability_unavailable', false),
  ]);
  await assert.rejects(read, { code: 'capability_unavailable' });
  assert.equal(requests(), 1);
  assert.deepEqual(delays, []);
  assert.equal(store.selectedPage(), null);
});

test('persistent publication failure stops after five automatic retries', async (t) => {
  t.mock.method(Math, 'random', () => 0.5);
  const { read, delays, requests } = fixture(
    t,
    Array(6).fill(failure(503, 'catalog_unavailable', true)),
  );
  for (let i = 0; i < 5; ++i) assert.equal(await read(), false);
  await assert.rejects(read, { code: 'catalog_unavailable' });
  assert.deepEqual(delays, [500, 1000, 2000, 4000, 8000]);
  assert.equal(requests(), 6);
});

test('503 without a retryable flag does not schedule an initial retry', async (t) => {
  const { read, delays, requests } = fixture(t, [failure(503, 'catalog_unavailable')]);
  await assert.rejects(read, { code: 'catalog_unavailable' });
  assert.deepEqual(delays, []);
  assert.equal(requests(), 1);
});

test('success from a stalled retry is ignored after the overall deadline', async (t) => {
  t.mock.timers.enable({ apis: ['Date', 'setTimeout'] });
  t.mock.method(Math, 'random', () => 0.5);
  const pending = failure(503, 'catalog_unavailable', true);
  const { store, read, delays, timeouts, recovery, requests } = fixture(t, [
    pending,
    pending,
    () => new Promise((resolve) => setTimeout(() => resolve([200, page]), 15_000)),
  ]);
  assert.equal(await read(), false);
  t.mock.timers.tick(14_000);
  assert.equal(await read(), false);
  t.mock.timers.tick(26_000);
  const stalled = read();
  assert.deepEqual(timeouts, [15_000, 15_000, 5_000]);
  t.mock.timers.tick(5_000);
  assert.equal(recovery.stopped, true);
  t.mock.timers.tick(10_000);
  assert.equal(await stalled, false);
  assert.deepEqual(delays, [500, 1000]);
  assert.equal(store.selectedPage(), null);
  assert.equal(await read(), false);
  assert.equal(requests(), 3);
});

test('full jitter samples different offsets across the backoff window', async (t) => {
  const samples = [0.1, 0.9];
  t.mock.method(Math, 'random', () => samples.shift());
  const pending = failure(503, 'catalog_unavailable', true);
  const { read, delays } = fixture(t, [pending, pending]);
  assert.equal(await read(), false);
  assert.equal(await read(), false);
  assert.deepEqual(delays, [100, 1800]);
});

test('an earlier retained page retries the same cursor after source preparation', async (t) => {
  const random = t.mock.method(Math, 'random', () => 0.15);
  const latest = {
    ...page,
    read_scope: 'retained_history',
    identity: { ...page.identity, read_scope: 'retained_history' },
    earlier_cursor: 'previous-window',
  };
  const earlierEntry = {
    ...page.entries[0],
    text: 'Earlier retained entry',
    entry_id: 'earlier-prompt',
    address: { ...page.entries[0].address, entry_id: 'earlier-prompt' },
  };
  const earlierPage = {
    ...latest,
    position: 'earlier',
    entries: [earlierEntry],
    earlier_cursor: null,
  };
  const { store, read, delays, timeouts, paths } = fixture(
    t,
    [[200, latest], failure(503, 'retained_source_preparing', true), [200, earlierPage]],
    'retained_history',
  );
  assert.equal(await read(), true);
  const window = { position: 'earlier', cursor: store.earlierCursor() };
  assert.equal(await read(window), false);
  assert.deepEqual(delays, [1000]);
  assert.equal(random.mock.callCount(), 0);
  assert.equal(await read(window), true);
  assert.equal(paths[1], paths[2]);
  assert.match(paths[2], /[&?]cursor=previous-window(?:&|$)/);
  assert.deepEqual(timeouts, [15_000, 15_000, 15_000]);
  assert.equal(store.entries()[0].entry.text, 'Earlier retained entry');
  assert.equal(store.earlierCursor(), null);
});

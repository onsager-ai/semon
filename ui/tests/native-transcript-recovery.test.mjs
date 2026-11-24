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

function fixture(t, answers) {
  let requests = 0;
  t.mock.method(globalThis, 'fetch', async () => {
    const [status, body] = answers[requests++];
    return Response.json(body, { status });
  });
  const store = new CatalogNativeTranscriptStore(selection, 'current');
  const delays = [];
  const recovery = new CatalogRecovery(5);
  const read = () =>
    readNativeTranscript(
      store,
      { position: 'latest' },
      {
        read: () => requestJson('/api/session-transcript?sid=example'),
        current: () => true,
        recovery,
        retry: (delay) => delays.push(delay),
      },
    );
  return { store, read, delays, requests: () => requests };
}

const failure = (status, code, retryable, progress) => [
  status,
  { error: { code, message: code, retryable, progress } },
];

test('the initial native transcript loads after history, Catalog and retained retries', async (t) => {
  const { store, read, delays, requests } = fixture(t, [
    failure(503, 'history_rebuilding', true, { done: 1, total: 4 }),
    failure(503, 'catalog_unavailable', true),
    failure(503, 'retained_source_preparing', true),
    [200, page],
  ]);
  const backoff = [1000, 2000, 4000];
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
  const { read, delays, requests } = fixture(
    t,
    Array(6).fill(failure(503, 'catalog_unavailable', true)),
  );
  for (let i = 0; i < 5; ++i) assert.equal(await read(), false);
  await assert.rejects(read, { code: 'catalog_unavailable' });
  assert.deepEqual(delays, [1000, 2000, 4000, 8000, 16000]);
  assert.equal(requests(), 6);
});

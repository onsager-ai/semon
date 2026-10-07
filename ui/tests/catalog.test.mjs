import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-wire.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseCatalogPage } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const session = () => ({
  key: 'stable-session',
  name: '<img src=x> A session',
  harness: 'codex',
  kind: 'Session',
  native_ids: ['native-session'],
  parent: 'parent-key',
  parent_source: 'source-native-id',
  repo: null,
  branch: null,
  model: 'model',
  effort: null,
  start: null,
  last: null,
  tokens: [0, 1, 2],
  cost: { usd: null, unpriced_models: ['model'] },
  source_refs: [
    {
      source: {
        root: 'codex',
        path: 'sessions/source.jsonl',
        native_id: 'native-session',
        offset: 8,
        prefix_sha256: Array(32).fill(1),
        tail_sha256: Array(32).fill(2),
      },
      state: 'unavailable',
    },
  ],
  freshness: { state: 'unavailable' },
});
const page = () => ({
  api: 1,
  machine: 'stable-machine',
  machine_info: { key: 'stable-machine', label: 'Display label', freshness: 'cached' },
  generation: '1'.repeat(64),
  observed_at: null,
  freshness: 'cached',
  capabilities: {
    pagination: true,
    filters: ['harness', 'repo'],
    order: 'last_desc_key_asc',
    full_text_search: false,
    selected_session_lookup: true,
    runtime_status: false,
    global_union: false,
  },
  items: [session()],
  next_cursor: 'opaque cursor',
});
test('catalog retains unknown timestamps, source disappearance and independent capabilities', () => {
  const input = page(),
    parsed = parseCatalogPage(input);
  assert.equal(parsed.observed_at, null);
  assert.equal(parsed.items[0].start, null);
  assert.equal(parsed.items[0].last, null);
  assert.equal(parsed.items[0].freshness.state, 'unavailable');
  assert.equal(parsed.items[0].source_refs[0].state, 'unavailable');
  assert.equal(parsed.capabilities.runtime_status, false);
  assert.equal(parsed.capabilities.global_union, false);
  assert.equal('state' in parsed.items[0], false);
  input.items[0].source_refs[0].source.prefix_sha256[0] = 255;
  input.items[0].name = 'mutated';
  input.capabilities.filters.push('private');
  assert.equal(parsed.items[0].source_refs[0].source.prefix_sha256[0], 1);
  assert.equal(parsed.items[0].name, '<img src=x> A session');
  assert.deepEqual(parsed.capabilities.filters, ['harness', 'repo']);
});
test('catalog rejects wrong machine identity, duplicate rows and malformed source generations', () => {
  for (const change of [
    (p) => (p.machine_info.key = 'another-machine'),
    (p) => p.items.push(session()),
    (p) => (p.items[0].source_refs[0].source.offset = Number.MAX_SAFE_INTEGER + 1),
    (p) => (p.items[0].source_refs[0].source.prefix_sha256 = [1]),
    (p) => (p.items[0].freshness.state = 'healthy'),
    (p) => (p.items[0].last = Infinity),
    (p) => (p.generation = 'unversioned'),
    (p) => (p.next_cursor = ''),
    (p) => (p.items = Array.from({ length: 101 }, (_, i) => ({ ...session(), key: String(i) }))),
  ]) {
    const input = page();
    change(input);
    assert.throws(() => parseCatalogPage(input));
  }
});

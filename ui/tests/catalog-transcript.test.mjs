import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-transcript-wire.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseCatalogTranscriptPage } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
const source = () => ({
  root: 'claude',
  path: 'native.jsonl',
  native_id: 'native',
  offset: 1,
  prefix_sha256: Array(32).fill(1),
  tail_sha256: Array(32).fill(2),
});
const provenance = () => ({ source: source(), native_event_id: null, offset: 42, block: 0 });
const page = () => ({
  api: 1,
  identity: {
    source_key: 'source',
    catalog_key: 'session',
    harness: 'claude',
    native_id: 'native',
    native_ids: ['native'],
    source_refs: [{ source: source(), state: 'cached' }],
    machine_label: null,
    generation: 'a'.repeat(64),
    observed_at: null,
    freshness: { state: 'cached' },
  },
  session: {
    key: 'session',
    name: 'Saved native session',
    harness: 'claude',
    kind: 'Claude Code',
    native_ids: ['native'],
    parent: null,
    parent_source: null,
    repo: null,
    branch: null,
    model: 'unknown',
    effort: null,
    start: null,
    last: null,
    tokens: [0, 0, 0],
    cost: { usd: null, unpriced_models: [] },
    source_refs: [{ source: source(), state: 'unavailable' }],
    freshness: { state: 'unavailable' },
  },
  projection: { version: 1, generation: 'b'.repeat(64), total: 3 },
  range: { first: 0, end: 2, next: 2 },
  entries: [
    {
      k: 'u',
      text: 'Original native text',
      slot: 0,
      entry_id: 'event-stable',
      provenance: provenance(),
    },
    {
      k: 'h',
      id: 'action-native',
      text: 'Original native action prompt',
      slot: 1,
      entry_id: 'action-stable',
      provenance: provenance(),
    },
  ],
  freshness: { state: 'incomplete' },
  relationship_context: { state: 'incomplete', turn_ids: ['native-turn'], handoffs: [] },
});
test('ranged native content preserves identity/provenance and incomplete action context', () => {
  const raw = page(),
    parsed = parseCatalogTranscriptPage(raw);
  assert.equal(parsed.session.start, null);
  assert.equal(parsed.identity.machine_label, null);
  assert.equal(parsed.entries[0].entry.key, 'event-stable');
  assert.equal(parsed.entries[1].native_action_text, 'Original native action prompt');
  assert.equal(parsed.entries[1].entry.id, 'action-native');
  assert.equal(parsed.relationship_context.state, 'incomplete');
  assert.equal(parsed.freshness.state, 'incomplete');
  assert.equal('state' in parsed.session, false);
  raw.entries[0].provenance.source.prefix_sha256[0] = 9;
  assert.equal(parsed.entries[0].provenance.source.prefix_sha256[0], 1);
  const appended = page();
  appended.projection.generation = 'c'.repeat(64);
  appended.projection.total = 4;
  assert.equal(
    parseCatalogTranscriptPage(appended).entries[0].entry.key,
    parsed.entries[0].entry.key,
  );
});
test('ranged native content rejects holes, stale scope and fabricated complete context', () => {
  for (const change of [
    (p) => (p.identity.catalog_key = 'other'),
    (p) => (p.session.harness = 'codex'),
    (p) => (p.entries[0].slot = 1),
    (p) => p.entries.pop(),
    (p) => (p.entries[1].entry_id = p.entries[0].entry_id),
    (p) => (p.range.next = null),
    (p) => (p.projection.generation = 'not-sha'),
    (p) => (p.relationship_context.state = 'complete'),
    (p) => (p.relationship_context.handoffs = [{}]),
    (p) => (p.entries[1].text = undefined),
    (p) => (p.entries[0].provenance.offset = Number.MAX_SAFE_INTEGER + 1),
    (p) => (p.freshness.state = 'fresh'),
  ]) {
    const p = page();
    change(p);
    assert.throws(() => parseCatalogTranscriptPage(p));
  }
  const p = page();
  p.projection.total = 2;
  p.range.next = null;
  assert.equal(parseCatalogTranscriptPage(p).range.next, null);
});
const { outputFiles: storeFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-transcript.ts', import.meta.url).pathname],
  bundle: true, write: false, format: 'esm', platform: 'node',
});
const { CatalogTranscriptStore } = await import(
  'data:text/javascript;base64,' + Buffer.from(storeFiles[0].text).toString('base64')
);
test('selected ranges reject old selections, cross-generation mixing and inconsistent overlaps atomically', () => {
  const store = new CatalogTranscriptStore();
  const scope = { source_key: 'source', catalog_key: 'session' };
  store.select(scope);
  const old = store.request(0, 2);
  store.select({ ...scope, catalog_key: 'other' });
  store.select(scope);
  assert.equal(store.accept(old, page()), false);
  const first = store.request(0, 2);
  assert.equal(store.accept(first, page()), true);
  assert.equal(store.accept(first, page()), false);
  assert.deepEqual(store.loadedRanges(), [{ first: 0, end: 2 }]);
  const before = store.entries();
  const bad = page();
  bad.projection.generation = 'c'.repeat(64);
  assert.throws(() => store.accept(store.request(0, 2), bad), /requested range/);
  assert.deepEqual(store.entries(), before);
  const overlap = page();
  overlap.entries[0].text = 'Edited within the same generation';
  assert.throws(() => store.accept(store.request(0, 2), overlap), /changed within/);
  assert.deepEqual(store.entries(), before);
  const tail = page();
  tail.range = { first: 2, end: 3, next: null };
  tail.entries = [{ k: 'a', text: 'Native final message', slot: 2, entry_id: 'final', provenance: provenance() }];
  assert.equal(store.accept(store.request(null, 1), tail), true);
  assert.deepEqual(store.loadedRanges(), [{ first: 0, end: 3 }]);
  assert.deepEqual(store.entries().map(item => item.entry_id), ['event-stable', 'action-stable', 'final']);
  const late = store.request(0, 2);
  store.destroy();
  assert.equal(store.accept(late, page()), false);
  assert.throws(() => store.request(), /No selected/);
});

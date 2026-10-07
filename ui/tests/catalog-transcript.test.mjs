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
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
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
  const observations = page();
  observations.identity.facts_observation = { state: 'cached' };
  observations.identity.read_scope = 'retained_history';
  observations.identity.native_selection = { state: 'retired' };
  observations.session.facts_observation = { state: 'unavailable' };
  observations.session.native_selection = { state: 'incomplete' };
  const observed = parseCatalogTranscriptPage(observations);
  assert.equal(observed.identity.facts_observation.state, 'cached');
  assert.equal(observed.identity.read_scope, 'retained_history');
  assert.equal(observed.identity.native_selection.state, 'retired');
  assert.equal(observed.session.facts_observation.state, 'unavailable');
  assert.equal(observed.session.native_selection.state, 'incomplete');
  observations.identity.facts_observation.state = 'invented-authority';
  assert.throws(() => parseCatalogTranscriptPage(observations));
  const clipped = page();
  clipped.entries[0].clipped = true;
  clipped.entries[0].field = { name: 'text', chunks: 3, complete: true };
  assert.equal(parseCatalogTranscriptPage(clipped).entries[0].field.chunks, 3);
  assert.equal(parseCatalogTranscriptPage(clipped).entries[0].clipped, true);
  clipped.entries[0].field.name = 'unknown';
  assert.throws(() => parseCatalogTranscriptPage(clipped));
  const version = store.revision;
  const refreshed = page();
  refreshed.identity.observed_at = 42;
  assert.equal(store.accept(store.request(0, 2), refreshed), true);
  assert.equal(store.revision, version);
  assert.equal(store.selectedPage().identity.observed_at, 42);
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
  tail.entries = [
    { k: 'a', text: 'Native final message', slot: 2, entry_id: 'final', provenance: provenance() },
  ];
  assert.equal(store.accept(store.request(null, 1), tail), true);
  assert.deepEqual(store.loadedRanges(), [{ first: 0, end: 3 }]);
  assert.deepEqual(
    store.entries().map((item) => item.entry_id),
    ['event-stable', 'action-stable', 'final'],
  );
  const late = store.request(0, 2);
  store.destroy();
  assert.equal(store.accept(late, page()), false);
  assert.throws(() => store.request(), /No selected/);
});
test('incomplete source reobservation retains useful body and exposes loss without accepting edits', () => {
  const store = new CatalogTranscriptStore();
  store.select({ source_key: 'source', catalog_key: 'session' });
  const initial = page();
  initial.entries[0].freshness = { state: 'cached' };
  store.accept(store.request(0, 2), initial);
  const lost = page();
  lost.entries[0].text = '';
  lost.entries[0].freshness = { state: 'incomplete' };
  const version = store.revision;
  assert.equal(store.accept(store.request(0, 2), lost), true);
  assert.equal(store.entries()[0].entry.text, 'Original native text');
  assert.equal(store.entries()[0].freshness.state, 'incomplete');
  assert.ok(store.revision > version);
  assert.equal(store.selectedPage().freshness.state, 'incomplete');
  assert.equal(store.accept(store.request(0, 2), initial), true);
  assert.equal(store.entries()[0].freshness.state, 'cached');
  const changed = page();
  changed.entries[0].text = '';
  changed.entries[0].freshness = { state: 'cached' };
  assert.throws(() => store.accept(store.request(0, 2), changed), /changed within/);
  lost.entries[0].provenance.offset++;
  assert.throws(() => store.accept(store.request(0, 2), lost), /changed within/);
  const empty = page();
  empty.entries[0].text = '';
  empty.entries[0].freshness = { state: 'cached' };
  const fresh = new CatalogTranscriptStore();
  fresh.select({ source_key: 'source', catalog_key: 'session' });
  fresh.accept(fresh.request(0, 2), empty);
  assert.equal(fresh.entries()[0].entry.text, '');
});
test('retained-history range intent cannot silently accept a current-scope projection', () => {
  const store = new CatalogTranscriptStore();
  store.select({ source_key: 'source', catalog_key: 'session' }, 'retained_history');
  assert.throws(() => store.accept(store.request(0, 2), page()), /requested range/);
  assert.equal(store.selectedPage(), null);
  const historical = page();
  historical.identity.read_scope = 'retained_history';
  historical.identity.native_selection = { state: 'retired' };
  assert.equal(store.accept(store.request(0, 2), historical), true);
  assert.equal(store.selectedPage().identity.native_selection.state, 'retired');
});

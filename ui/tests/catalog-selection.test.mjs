import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-selection.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { CatalogSelectionStore } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
const ref = (native_id) => ({
  source: {
    root: 'claude',
    path: native_id + '.jsonl',
    native_id,
    offset: 42,
    prefix_sha256: Array(32).fill(1),
    tail_sha256: Array(32).fill(2),
  },
  state: 'cached',
});
const identity = (key = 'session', ids = ['native']) => ({
  source_key: 'source',
  catalog_key: key,
  harness: 'claude',
  native_id: ids.length === 1 ? ids[0] : null,
  native_ids: ids,
  source_refs: ids.map(ref),
  machine_label: null,
  generation: 'a'.repeat(64),
  observed_at: null,
  freshness: { state: 'cached' },
});
test('catalog identity has its own selection epoch and no graph/runtime defaults', () => {
  const store = new CatalogSelectionStore();
  for (const scope of [
    { source_key: 1, catalog_key: 'session' },
    { source_key: '', catalog_key: 'session' },
    { source_key: 'source', catalog_key: 42 },
    { source_key: 'source', catalog_key: '' },
  ])
    assert.throws(() => store.begin(scope));
  let changes = 0;
  const unsubscribe = store.subscribe(() => changes++);
  const first = store.begin({ source_key: 'source', catalog_key: 'session' });
  const second = store.begin({ source_key: 'source', catalog_key: 'other' });
  const third = store.begin({ source_key: 'source', catalog_key: 'session' });
  assert.equal(store.accept(first, identity()), false);
  assert.equal(store.accept(second, identity('other')), false);
  assert.equal(store.selectedIdentity(), null);
  const wire = identity();
  assert.equal(store.accept(third, wire), true);
  const accepted = store.selectedIdentity();
  assert.equal(accepted.native_id, 'native');
  assert.equal(accepted.observed_at, null);
  assert.equal(accepted.machine_label, null);
  assert.equal('state' in accepted, false);
  assert.equal('up' in accepted, false);
  wire.source_refs[0].source.native_id = 'changed';
  assert.equal(accepted.source_refs[0].source.native_id, 'native');
  assert.equal(store.selectedIdentity(), accepted);
  assert.throws(() => {
    accepted.source_refs[0].source.prefix_sha256[0] = 9;
  });
  store.clear();
  assert.equal(store.selectedIdentity(), null);
  assert.equal(store.accept(third, identity()), false);
  assert.equal(changes, 5);
  unsubscribe();
  store.clear();
  assert.equal(changes, 5);
});
test('catalog control ambiguity remains explicit and mismatched provenance is rejected atomically', () => {
  const store = new CatalogSelectionStore();
  const request = store.begin({ source_key: 'source', catalog_key: 'session' });
  assert.equal(store.accept(request, identity('session', ['one', 'two'])), true);
  assert.equal(store.selectedIdentity().native_id, null);
  for (const bad of [
    { ...identity(), source_key: 'wrong' },
    { ...identity(), catalog_key: 'other' },
    { ...identity('session', ['one', 'two']), native_id: 'one' },
    { ...identity(), native_ids: ['other'] },
    { ...identity(), generation: 'bad' },
  ])
    assert.throws(() => store.accept(request, bad));
  assert.equal(store.selectedIdentity().native_id, null);
  assert.equal(store.selectedIdentity().source_refs.length, 2);
});
test('selection subscribers cannot turn an old begin into a newer ticket', () => {
  const store = new CatalogSelectionStore();
  const remove = store.subscribe(() => store.clear());
  // Remove the listener before clear notifies again; simulate one synchronous disposal.
  remove();
  let disposed = false;
  const unsubscribe = store.subscribe(() => {
    if (!disposed) {
      disposed = true;
      store.clear();
    }
  });
  const request = store.begin({ source_key: 'source', catalog_key: 'session' });
  assert.equal(store.accept(request, identity()), false);
  assert.equal(store.selectedIdentity(), null);
  unsubscribe();
});

test('catalog selection lifetime invalidates outstanding replies and subscriptions', () => {
  const store = new CatalogSelectionStore();
  let changes = 0;
  store.subscribe(() => changes++);
  const request = store.begin({ source_key: 'source', catalog_key: 'session' });
  store.destroy();
  assert.equal(store.accept(request, identity()), false);
  assert.equal(store.selectedIdentity(), null);
  assert.equal(changes, 1);
  assert.throws(() => store.begin({ source_key: 'source', catalog_key: 'session' }));
  assert.throws(() => store.subscribe(() => {}));
  store.destroy();
});
test('retained or retired native identities cannot grant current controls; byte loss stays separate', () => {
  const store = new CatalogSelectionStore(),
    ticket = store.begin({ source_key: 'source', catalog_key: 'session' });
  assert.throws(
    () => store.accept(ticket, { ...identity(), read_scope: 'retained_history' }),
    /current catalog authority/,
  );
  assert.throws(
    () => store.accept(ticket, { ...identity(), native_selection: { state: 'retired' } }),
    /current catalog authority/,
  );
  assert.equal(store.selectedIdentity(), null);
  const unavailable = identity();
  unavailable.freshness = { state: 'unavailable' };
  unavailable.source_refs[0].state = 'unavailable';
  unavailable.facts_observation = { state: 'cached' };
  unavailable.native_selection = { state: 'cached' };
  assert.equal(store.accept(ticket, unavailable), true);
  assert.equal(store.selectedIdentity().freshness.state, 'unavailable');
  assert.equal(store.selectedIdentity().facts_observation.state, 'cached');
  assert.ok(Object.isFrozen(store.selectedIdentity().facts_observation));
  assert.ok(Object.isFrozen(store.selectedIdentity().native_selection));
  store.destroy();
});
test('readonly scope exists before authority and remains stable through failed identity rechecks', () => {
  const store = new CatalogSelectionStore();
  assert.equal(store.selectedScope(), null);
  const first = store.begin({ source_key: 'source', catalog_key: 'session' });
  const scope = store.selectedScope();
  assert.deepEqual(scope, { source_key: 'source', catalog_key: 'session' });
  assert.ok(Object.isFrozen(scope));
  store.accept(first, identity());
  const retry = store.begin(scope);
  assert.equal(store.selectedIdentity(), null);
  assert.equal(store.selectedScope(), scope);
  assert.throws(() => store.accept(retry, { ...identity(), read_scope: 'retained_history' }));
  assert.equal(store.selectedScope(), scope);
  store.begin({ source_key: 'other-source', catalog_key: 'session' });
  assert.notEqual(store.selectedScope(), scope);
  assert.equal(store.accept(retry, identity()), false);
  store.clear();
  assert.equal(store.selectedScope(), null);
  store.begin(scope);
  store.destroy();
  assert.equal(store.selectedScope(), null);
});

test('provisional identity retains read provenance without native control authority', () => {
  const store = new CatalogSelectionStore(),
    ticket = store.begin({ source_key: 'source', catalog_key: 'session' });
  const provisional = { ...identity(), owner_qualification: 'provisional', native_id: null };
  assert.equal(store.accept(ticket, provisional), true);
  assert.equal(store.selectedIdentity().native_id, null);
  assert.deepEqual(store.selectedIdentity().native_ids, ['native']);
  assert.equal(store.selectedScope().catalog_key, 'session');
  const next = store.begin({ source_key: 'source', catalog_key: 'session' });
  assert.throws(() => store.accept(next, { ...provisional, native_id: 'native' }));
  assert.throws(() => store.accept(next, { ...provisional, owner_qualification: 'unknown' }));
});

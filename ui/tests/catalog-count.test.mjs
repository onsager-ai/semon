import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';

const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-count.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { catalogCountWitness, completeCatalogCount } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
const filters = (query = '') => JSON.stringify(['', '', '', query]);
function observation(keys = ['a', 'b'], source = 'laptop') {
  const page = {
    machine: source,
    generation: 'a'.repeat(64),
    read_scope: 'retained_history',
    completeness: { state: 'complete' },
    items: keys.map((key) => ({ key })),
    next_cursor: null,
    search: null,
  };
  return {
    page,
    items: [...page.items],
    witness: catalogCountWitness(page, filters(), true),
    source,
    filters: filters(),
    searchRequested: false,
    updating: false,
    more: false,
  };
}

test('only a complete unpaged exact source observation establishes the displayed count', () => {
  for (const keys of [[], ['a'], ['a', 'b'], Array.from({ length: 100 }, (_, i) => String(i))]) {
    const value = observation(keys);
    assert.equal(completeCatalogCount(value), true);
    value.items.reverse();
    assert.equal(completeCatalogCount(value), true, 'ordering does not change membership');
  }
  assert.equal(
    completeCatalogCount(observation(Array.from({ length: 101 }, (_, i) => String(i)))),
    false,
  );
});

test('failed filter and query observations cannot relabel retained rows as a complete result', () => {
  const value = observation();
  value.updating = true;
  assert.equal(completeCatalogCount(value), false);
  value.updating = false;
  // A failed request leaves the prior accepted page and witness untouched.
  for (const changed of [filters('needle'), JSON.stringify(['codex', 'repo', 'model', ''])]) {
    value.filters = changed;
    assert.equal(completeCatalogCount(value), false);
  }
  value.filters = filters();
  assert.equal(completeCatalogCount(value), true);
  value.filters = filters('needle');
  value.searchRequested = true;
  value.witness = catalogCountWitness(value.page, value.filters, true);
  assert.equal(completeCatalogCount(value), false, 'an ignored query has no accepted search proof');
  value.page.search = { partial: false, index_complete: true };
  assert.equal(
    completeCatalogCount(value),
    true,
    'an accepted complete filtered result is bounded',
  );
});

test('partial discovery, bounded search and pagination keep loaded wording', () => {
  for (const change of [
    (value) => (value.page.completeness.state = 'partial'),
    (value) => (value.page.search = { partial: true, index_complete: true }),
    (value) => (value.page.search = { partial: false, index_complete: false }),
    (value) => (value.page.next_cursor = 'next-page'),
    (value) => (value.more = true),
    // A final cursor page is not proof of the full result, even with matching keys.
    (value) => (value.witness = catalogCountWitness(value.page, value.filters, false)),
  ]) {
    const value = observation();
    change(value);
    assert.equal(completeCatalogCount(value), false);
  }
});

test('retained old keys, missing rows and duplicate identities never establish a census', () => {
  for (const keys of [['a', 'old'], ['a'], ['a', 'a'], ['a', 'b', 'old']]) {
    const value = observation();
    value.items = keys.map((key) => ({ key }));
    assert.equal(completeCatalogCount(value), false);
  }
  const duplicatePage = observation(['a', 'a']);
  assert.equal(completeCatalogCount(duplicatePage), false);
});

test('a page cannot reuse evidence from another generation, source or read scope', () => {
  for (const change of [
    (value) => (value.page.generation = 'b'.repeat(64)),
    (value) => (value.source = 'workstation'),
    (value) => (value.page.machine = 'workstation'),
    (value) => (value.page.read_scope = 'current'),
    (value) => (value.witness = null),
    (value) => (value.page = null),
  ]) {
    const value = observation();
    change(value);
    assert.equal(completeCatalogCount(value), false);
  }
});

test('per-source restoration preserves its accepted evidence without crossing sources or filters', () => {
  const laptop = observation(['a', 'b'], 'laptop'),
    workstation = observation(['c'], 'workstation'),
    retained = new Map([
      ['laptop', laptop],
      ['workstation', workstation],
    ]);
  for (const source of ['laptop', 'workstation', 'laptop']) {
    const saved = retained.get(source);
    assert.equal(completeCatalogCount({ ...saved, source }), true);
    assert.equal(completeCatalogCount({ ...saved, source, filters: filters('new query') }), false);
    assert.equal(
      completeCatalogCount({ ...saved, source: source === 'laptop' ? 'workstation' : 'laptop' }),
      false,
    );
  }
});

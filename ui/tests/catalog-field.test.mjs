import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-field-wire.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseCatalogField } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
const source = {
  root: 'claude',
  path: 'native.jsonl',
  native_id: 'native',
  offset: 0,
  prefix_sha256: Array(32).fill(1),
  tail_sha256: Array(32).fill(2),
};
const provenance = { source, offset: 10, block: 0, native_event_id: null };
const request = {
  source_key: 'source',
  catalog_key: 'session',
  generation: 'b'.repeat(64),
  chunk: 0,
  entry: { slot: 2, field: { name: 'out', chunks: 2, complete: false }, provenance },
};
function body() {
  return {
    api: 1,
    identity: {
      source_key: 'source',
      catalog_key: 'session',
      read_scope: 'retained_history',
      harness: 'claude',
      native_id: 'native',
      native_ids: ['native'],
      source_refs: [{ source, state: 'cached' }],
      machine_label: null,
      generation: 'a'.repeat(64),
      observed_at: null,
      freshness: { state: 'cached' },
    },
    projection: { version: 5, generation: request.generation },
    slot: 2,
    field: { name: 'out', chunk: 0, next: 1, complete: false },
    text: 'First native result chunk',
    freshness: { state: 'cached' },
    provenance: structuredClone(provenance),
    observation: { source_bytes: 25 },
  };
}
test('native field chunks bind source, canonical session, projection, slot, field and provenance', () => {
  assert.deepEqual(parseCatalogField(body(), request), {
    text: 'First native result chunk',
    next: 1,
    complete: false,
  });
  const last = body();
  last.field = { name: 'out', chunk: 1, next: null, complete: true };
  last.text = 'Final chunk';
  assert.deepEqual(parseCatalogField(last, { ...request, chunk: 1 }), {
    text: 'Final chunk',
    next: null,
    complete: true,
  });
  const layeredRequest = structuredClone(request);
  layeredRequest.entry.field.layers = 2;
  const layered = body();
  layered.field.layers = 2;
  layered.observation.source_bytes = 65607;
  assert.equal(parseCatalogField(layered, layeredRequest).text, layered.text);
  layered.observation.source_bytes = 65608;
  assert.throws(() => parseCatalogField(layered, layeredRequest));
  for (const change of [
    (p) => (p.identity.source_key = 'other'),
    (p) => (p.identity.catalog_key = 'other'),
    (p) => (p.identity.read_scope = 'current'),
    (p) => (p.projection.version = 1),
    (p) => (p.projection.version = 6),
    (p) => (p.projection.generation = 'c'.repeat(64)),
    (p) => (p.slot = 3),
    (p) => (p.field.name = 'text'),
    (p) => (p.field.chunk = 1),
    (p) => (p.field.next = 3),
    (p) => (p.field.complete = true),
    (p) => p.provenance.offset++,
    (p) => (p.provenance.source.native_id = 'other'),
    (p) => (p.freshness.state = 'incomplete'),
    (p) => (p.observation.source_bytes = 65548),
    (p) => (p.text = 'x'.repeat(131073)),
    (p) => (p.field.layers = 2),
    (p) => (p.field.layers = 3),
  ]) {
    const p = body();
    change(p);
    assert.throws(() => parseCatalogField(p, request));
  }
});

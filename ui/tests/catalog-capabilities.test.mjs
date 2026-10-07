import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-capabilities.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseCatalogCapabilities } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
const capabilities = () => ({
  api: 1,
  read_contract: 'catalog-v1',
  source_key: 'local',
  selected_transcript: false,
  selected_identity: true,
  selected_entry: false,
  attachment: false,
  relationship_context: false,
  large_native_records: false,
  pagination: true,
  filters: ['harness', 'repo'],
  order: 'last_desc_key_asc',
  full_text_search: false,
  global_union: false,
});
test('catalog capability advertisement preserves unavailable features and rejects unsupported scope contracts', () => {
  const parsed = parseCatalogCapabilities(capabilities());
  assert.equal(parsed.source_key, 'local');
  assert.equal(parsed.selected_transcript, false);
  assert.equal(parsed.selected_entry, false);
  for (const change of [
    { api: 2 },
    { read_contract: 'future' },
    { source_key: null },
    { source_key: '' },
    { selected_transcript: undefined },
    { pagination: false },
    { filters: ['q'] },
    { order: 'global' },
    { full_text_search: true },
    { global_union: true },
  ])
    assert.throws(() => parseCatalogCapabilities({ ...capabilities(), ...change }));
});

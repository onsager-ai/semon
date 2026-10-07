import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/state/catalog-source-candidates.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseCatalogSourceCandidates, parseCatalogSourceCandidateProgress } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const candidate = () => ({
  candidate_key: 'opaque',
  source: { root: 'claude', path: 'project/native.jsonl' },
  native_name_hint: null,
  generation: 'a'.repeat(64),
  archive_observed_at: null,
});
test('archive hints preserve unknown verification time without inventing native session authority', () => {
  const input = { api: 1, source_key: 'source', items: [candidate()], next_cursor: null };
  const parsed = parseCatalogSourceCandidates(input);
  assert.equal(parsed.items[0].archive_observed_at, null);
  assert.equal('catalog_key' in parsed.items[0], false);
  assert.equal('native_id' in parsed.items[0], false);
  input.items[0].source.path = 'mutated';
  assert.equal(parsed.items[0].source.path, 'project/native.jsonl');
  for (const path of ['/absolute', '../parent', 'path/../parent', 'path\\\\host', 'path\0suffix'])
    assert.throws(() =>
      parseCatalogSourceCandidates({
        ...input,
        items: [{ ...candidate(), source: { root: 'claude', path } }],
      }),
    );
  assert.throws(() =>
    parseCatalogSourceCandidates({ ...input, items: [{ ...candidate(), generation: 'old' }] }),
  );
  assert.throws(() =>
    parseCatalogSourceCandidates({
      ...input,
      items: [{ ...candidate(), archive_observed_at: 1.5 }],
    }),
  );
});
test('archive selection accepts canonical identity only after parsed readiness', () => {
  const base = {
    api: 1,
    source_key: 'source',
    candidate_key: 'opaque',
    generation: 'a'.repeat(64),
    state: 'updating',
    catalog_key: null,
    retryable: true,
    reason: null,
  };
  assert.equal(parseCatalogSourceCandidateProgress(base).catalog_key, null);
  assert.equal(
    parseCatalogSourceCandidateProgress({ ...base, state: 'ready', catalog_key: 'parsed' })
      .catalog_key,
    'parsed',
  );
  assert.throws(() => parseCatalogSourceCandidateProgress({ ...base, catalog_key: 'guessed' }));
  assert.throws(() => parseCatalogSourceCandidateProgress({ ...base, state: 'ready' }));
  assert.throws(() => parseCatalogSourceCandidateProgress({ ...base, retryable: 'true' }));
});

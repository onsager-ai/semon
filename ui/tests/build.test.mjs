import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { buildViewer, assertFresh } from '../build.mjs';
test('production builds are deterministic and stale assets fail', async () => {
  const first = await buildViewer(), second = await buildViewer();
  const bytes = first.outputFiles[0].contents;
  assert.deepEqual(bytes, second.outputFiles[0].contents);
  await assertFresh(bytes, new URL('../../crates/semon-sessions/src/viewer.generated.js', import.meta.url));
  const dir = await mkdtemp(join(tmpdir(), 'semon-ui-'));
  try {
    const file = join(dir, 'viewer.generated.js');
    await writeFile(file, Buffer.concat([Buffer.from(bytes), Buffer.from('\n// stale')]));
    await assert.rejects(assertFresh(bytes, file), /Stale viewer.generated.js/);
  } finally { await rm(dir, { recursive: true }); }
  const inputs = Object.keys(first.metafile.inputs);
  assert.ok(inputs.some(p => p.endsWith('preact/dist/preact.module.js')));
  assert.ok(inputs.every(p => !/preact\/(debug|devtools|compat)\//.test(p)));
});

test('consumer build uses pinned types/security/runtime and rejects stale output', async () => {
  const { buildConsumer } = await import('../build.mjs');
  const dir = await mkdtemp(join(tmpdir(), 'semon-consumer-'));
  try {
    const entry = join(dir, 'consumer.ts'), output = join(dir, 'consumer.js');
    const library = new URL('../src/lib/index.ts', import.meta.url).pathname;
    await writeFile(entry, `import { createRecentRenderer } from ${JSON.stringify(library)}; console.log(createRecentRenderer);`);
    const result = await buildConsumer(entry, output);
    assert.equal(Object.keys(result.metafile.inputs).filter(p => /preact\/dist\/preact\.module\.js$/.test(p)).length, 1);
    await buildConsumer(entry, output, true);
    await writeFile(output, '// stale');
    await assert.rejects(buildConsumer(entry, output, true), /Stale consumer/);
    await writeFile(entry, 'const value: number = "wrong";');
    await assert.rejects(buildConsumer(entry, output), /not assignable/);
    await writeFile(entry, 'document.body.innerHTML = "unsafe";');
    await assert.rejects(buildConsumer(entry, output), /forbidden application boundary/);
  } finally { await rm(dir, { recursive: true }); }
});

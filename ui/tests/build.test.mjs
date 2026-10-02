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

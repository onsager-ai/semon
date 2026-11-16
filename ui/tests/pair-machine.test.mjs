import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/app/pairMachine.tsx', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parsePairingCode } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
test('pairing response validates the printable one-time code and expiry', () => {
  assert.deepEqual(parsePairingCode({ code: '01234-5678X', expires: 600 }), {
    code: '01234-5678X',
    expires: 600,
  });
  for (const value of [
    null,
    {},
    { code: '$(command)', expires: 600 },
    { code: '01234-5678X', expires: '600' },
    { code: '01234-5678X', expires: -1 },
  ])
    assert.throws(() => parsePairingCode(value));
});

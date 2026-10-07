import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/domain/resume.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { resumeCommand } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);

test('resume commands select the harness and native identity rather than a namespaced key', () => {
  for (const [harness, expected] of [
    ['claude', 'claude --resume native-id'],
    ['codex', 'codex resume native-id'],
    ['copilot', 'copilot --resume=native-id'],
  ]) {
    assert.equal(
      resumeCommand({ harness, id: 'machine:display-key', sessionId: 'native-id' }),
      expected,
    );
    assert.equal(resumeCommand({ harness, id: 'native-id' }), expected);
  }
});

test('unsupported harnesses and empty identities have no resume action', () => {
  for (const harness of ['cursor', 'opencode', '', 'other'])
    assert.equal(resumeCommand({ harness, id: 'native-id' }), null);
  for (const id of ['', '   ', '-option', 'line\nbreak', 'null\0byte'])
    assert.equal(resumeCommand({ harness: 'copilot', id }), null);
});

test('identities containing shell syntax stay a single literal argument', () => {
  assert.equal(
    resumeCommand({ harness: 'copilot', id: "native'$(echo injected)" }),
    "copilot --resume='native'\"'\"'$(echo injected)'",
  );
});

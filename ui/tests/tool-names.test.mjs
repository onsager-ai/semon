import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const bundle = await build({
  entryPoints: [new URL('../src/domain/toolNames.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
});
const { toolInfo } = await import(
  'data:text/javascript;base64,' + Buffer.from(bundle.outputFiles[0].text).toString('base64')
);
test('built-in app names use the same provider/action in dotted and MCP formats', () => {
  for (const name of [
    'codex_apps.github.fetch_pr',
    'mcp__codex_apps__github_fetch_pr',
    'mcp__github__fetch_pr',
    'mcp__claude_ai_github__fetch_pr',
    'mcp__claude_ai_GitHub__fetch_pr',
  ])
    assert.equal(toolInfo(name)[1], 'GitHub · fetch pr');
  assert.equal(toolInfo('mcp__codex_apps__google_drive_fetch')[1], 'Google Drive · fetch');
  assert.equal(toolInfo('exec_command')[1], 'Ran');
});
test('unknown tool and app identities remain distinguishable', () => {
  for (const name of [
    'custom',
    'mcp__codex_apps__unfamiliar_fetch',
    'codex_apps.unfamiliar.fetch',
    'mcp__server__some_action',
    'mcp__server__some__action',
  ]) {
    assert.equal(toolInfo(name)[1], name);
    if (name.startsWith('mcp__') || name.startsWith('codex_apps.'))
      assert.equal(toolInfo(name)[0], 'ext');
  }
});

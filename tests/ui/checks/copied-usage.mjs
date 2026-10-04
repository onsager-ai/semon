// Native mock evidence, copied only into a disposable home. Exercise served
// production assets, query evidence and unknown-cost presentation on both themes.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { served, reporter } from '../lib.mjs';

const repo = fileURLToPath(new URL('../../../', import.meta.url));
const now = 1_791_072_000_000;
function start(root) {
  const bin = process.env.SEMON_BIN ?? path.join(repo, 'target/debug/semon');
  const proc = spawn(bin, ['sessions', '--serve', '--listen', '127.0.0.1:0', '--claude-home', path.join(root, 'claude'), '--claude-json', path.join(root, '.claude.json'), '--codex-home', path.join(root, 'codex'), '--proc-root', path.join(root, 'proc'), '--cache', path.join(root, 'cache/view.json')], { env: { ...process.env, SEMON_TEST_NOW: String(now) }, stdio: ['ignore', 'pipe', 'pipe'] });
  return new Promise((resolve, reject) => {
    let log = '';
    const fail = (error) => { clearTimeout(timer); proc.kill(); reject(error); };
    const timer = setTimeout(() => fail(new Error('Copied usage fixture server did not start: ' + log)), 20_000);
    const read = (bytes) => {
      log += bytes;
      const match = /http:\/\/127\.0\.0\.1:(\d+)\/\?t=([a-f0-9]+)/.exec(log);
      if (match) { clearTimeout(timer); resolve({ proc, base: 'http://127.0.0.1:' + match[1], token: match[2] }); }
    };
    proc.stdout.on('data', read); proc.stderr.on('data', read);
    proc.on('error', fail);
    proc.on('exit', (code) => fail(new Error('Copied usage fixture server exited ' + code + ': ' + log)));
  });
}
export default async function copiedUsageCheck(browser) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'semon-copied-usage-'));
  const project = path.join(root, 'claude/projects/fixture');
  fs.mkdirSync(project, { recursive: true });
  const sources = new Map();
  for (const [id, snapshot] of [['native-claude-parent', 'resumed'], ['native-claude-child', 'forked']]) {
    const bytes = fs.readFileSync(path.join(repo, 'tests/fixtures/compatibility/claude-2.1.288/lifecycle/' + snapshot + '-transcript.jsonl'));
    const file = path.join(project, id + '.jsonl');
    fs.writeFileSync(file, bytes); sources.set(file, bytes);
  }
  let server;
  const report = reporter('copied-usage');
  try {
    server = await start(root);
    const model = await (await fetch(server.base + '/api/model?t=' + server.token)).json();
    const parent = model.sessions['native-claude-parent'], child = model.sessions['native-claude-child'];
    assert.equal(parent.claude_usage.observed.input + child.claude_usage.observed.input, 25);
    assert.equal(child.claude_usage.exclusive.input, 5);
    assert.equal(child.claude_usage.fresh, null);
    assert.equal(child.claude_usage.shared_owner, null);
    assert.deepEqual(parent.claude_usage.shared, child.claude_usage.shared);
    assert.equal(parent.cost.usd, null); assert.equal(child.cost.usd, null);
    for (const size of ['phone', 'desktop']) for (const dark of [false, true]) {
      const page = await served(browser, { ...server, size, dark, path: '/s/claude/native-claude-child' });
      try {
        await page.click('#more-btn');
        await page.waitForFunction(() => document.querySelector('dialog.session-menu')?.open);
        assert.equal(await page.locator('dialog.session-menu .cost-big').textContent(), '—');
        assert.match(await page.locator('dialog.session-menu .cost-note').first().textContent(), /Copied usage is excluded.*original owner.*unknown/);
        assert.deepEqual(page.errors, []);
        report.expect(true, size + (dark ? '-dark' : '-light') + ': copied usage retains unknown cost');
      } finally { await page.context().close(); }
    }
    for (const [file, bytes] of sources) assert.deepEqual(fs.readFileSync(file), bytes);
    report.results = { physicalSessions: 2, observedInput: 25, distinctInput: 15, exclusiveInput: 5, owner: null };
    return report.done();
  } finally {
    if (server) server.proc.kill();
    fs.rmSync(root, { recursive: true, force: true });
  }
}

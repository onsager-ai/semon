// Isolated real native logs for canonical transport, lifecycle and performance checks.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { write } from './fixture.mjs';
import { context } from './lib.mjs';

export async function nativeFixture({ prepare } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'semon-native-reader-'));
  const now = write(root);
  const prepared = await prepare?.(root, now);
  const binary =
    process.env.SEMON_BIN ?? new URL('../../target/debug/semon', import.meta.url).pathname;
  const proc = spawn(
    binary,
    [
      'sessions',
      '--serve',
      '--listen',
      '127.0.0.1:0',
      '--claude-home',
      path.join(root, 'claude'),
      '--claude-json',
      path.join(root, '.claude.json'),
      '--codex-home',
      path.join(root, 'codex'),
      '--copilot-home',
      path.join(root, 'copilot'),
      '--proc-root',
      path.join(root, 'proc'),
      '--cache',
      path.join(root, 'index.json'),
    ],
    { env: { ...process.env, SEMON_TEST_NOW: String(now) }, stdio: ['ignore', 'pipe', 'pipe'] },
  );
  let output = '';
  const capture = (value) => (output = (output + value).slice(-16384));
  proc.stdout.on('data', capture);
  proc.stderr.on('data', capture);
  let address;
  for (let n = 0; n < 200; n++) {
    address = output.match(/(http:\/\/127\.0\.0\.1:\d+)\/\?t=([a-f0-9]+)/);
    if (address) break;
    if (proc.exitCode !== null) throw Error('Native fixture exited: ' + output);
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  assert.ok(address, 'Native fixture did not start');
  const base = address[1],
    landing = base + '/?t=' + address[2];
  const bootstrap = await fetch(landing);
  const headers = {
    cookie: bootstrap.headers
      .getSetCookie()
      .map((value) => value.split(';')[0])
      .join('; '),
  };
  const request = (url) => fetch(base + url, { headers });
  let published = false;
  for (let n = 0; n < 200; n++) {
    const home = await (await request('/api/viewer/home')).json();
    if (home.recent?.length && home.observations?.length === 0) {
      published = true;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (!published) {
    proc.kill('SIGTERM');
    throw Error('Native fixture did not finish Source publication');
  }
  function fileFor(sid) {
    const files = fs.readdirSync(path.join(root, 'claude', 'projects'), { recursive: true });
    const rel = files.find((value) => path.basename(value) === sid + '.jsonl');
    assert.ok(rel, 'Fixture has a native file for ' + sid);
    return path.join(root, 'claude', 'projects', rel);
  }
  return {
    root,
    now,
    base,
    request,
    prepared,
    diagnostics: () => output.replaceAll(address[2], '[redacted]'),
    append(sid, text, at = now - 1000) {
      fs.appendFileSync(
        fileFor(sid),
        JSON.stringify({
          type: 'assistant',
          sessionId: sid,
          timestamp: new Date(at).toISOString(),
          uuid: 'native-live-' + crypto.randomUUID(),
          message: {
            role: 'assistant',
            model: 'claude-opus-5-5',
            content: [{ type: 'text', text }],
          },
        }) + '\n',
      );
    },
    async open(browser, opts = {}) {
      const owner = await context(browser, opts),
        page = await owner.newPage();
      page.errors = [];
      page.on('pageerror', (error) => page.errors.push(error.message));
      page.setDefaultTimeout(10000);
      await page.clock.setFixedTime(now);
      await page.goto(landing);
      return page;
    },
    async close() {
      if (proc.exitCode === null) {
        proc.kill('SIGTERM');
        await new Promise((resolve) => {
          const timeout = setTimeout(() => {
            proc.kill('SIGKILL');
            resolve();
          }, 3000);
          proc.once('exit', () => {
            clearTimeout(timeout);
            resolve();
          });
        });
      }
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

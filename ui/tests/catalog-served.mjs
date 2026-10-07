import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, writeFile, readFile, rm, readdir, appendFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
import { test } from 'node:test';
const root = new URL('../../', import.meta.url).pathname;
test(
  'real bounded producer cold list and retained selected range',
  { skip: !process.env.SEMON_CATALOG_SERVED_BIN },
  async () => {
    const home = await mkdtemp(join(tmpdir(), 'semon-real-catalog-'));
    const fixture = spawnSync(
      process.execPath,
      [join(root, 'tests/ui/fixture.mjs'), home, '--extras'],
      { encoding: 'utf8' },
    );
    assert.equal(fixture.status, 0, fixture.stderr);
    const paths = await readdir(join(home, 'claude'), { recursive: true });
    const backlog = paths.find((path) => path.endsWith('/backlog.jsonl'));
    assert.ok(backlog, 'Native long conversation fixture is missing');
    const scalar = 'Actual native scalar field.\n\n' + 'Recorded source text. '.repeat(5000);
    await appendFile(
      join(home, 'claude', backlog),
      JSON.stringify({
        type: 'assistant',
        sessionId: 'backlog',
        uuid: 'semon-served-scalar',
        timestamp: new Date(Number(fixture.stdout.trim()) - 1000).toISOString(),
        message: {
          role: 'assistant',
          content: [{ type: 'text', text: scalar }],
          model: 'claude-sonnet-5',
          usage: { input_tokens: 0, output_tokens: 0 },
        },
      }) + '\n',
    );
    const server = spawn(
      process.env.SEMON_CATALOG_SERVED_BIN,
      [
        'sessions',
        '--serve',
        '--listen',
        '127.0.0.1:0',
        '--claude-home',
        join(home, 'claude'),
        '--claude-json',
        join(home, '.claude.json'),
        '--codex-home',
        join(home, 'codex'),
        '--copilot-home',
        join(home, 'copilot'),
        '--proc-root',
        join(home, 'proc'),
        '--cache',
        join(home, 'index.json'),
      ],
      {
        env: { ...process.env, SEMON_TEST_NOW: fixture.stdout.trim() },
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    );
    let output = '';
    for (const stream of [server.stdout, server.stderr])
      stream.on('data', (data) => (output += data));
    const browser = await chromium.launch();
    try {
      let url;
      for (let i = 0; i < 200; i++) {
        url = output.match(/(http:\/\/127\.0\.0\.1:\d+)\/\?t=([a-f0-9]+)/);
        if (url) break;
        await new Promise((r) => setTimeout(r, 50));
      }
      assert.ok(url, output);
      const page = await browser.newPage({ viewport: { width: 1280, height: 860 } }),
        errors = [],
        requests = [],
        responses = [],
        fieldTexts = [];
      page.on('pageerror', (e) => errors.push(e.message));
      page.on('request', (r) => {
        if (new URL(r.url()).pathname.startsWith('/api/'))
          requests.push(new URL(r.url()).pathname + new URL(r.url()).search);
      });
      page.on('response', async (r) => {
        if (new URL(r.url()).pathname.startsWith('/api/')) {
          const t = r.request().timing();
          let body = '';
          try {
            body = await r.text();
          } catch {}
          if (new URL(r.url()).pathname === '/api/session-entry' && r.status() === 200) {
            const field = JSON.parse(body);
            fieldTexts[field.field.chunk] = field.text;
          }
          responses.push({
            path: new URL(r.url()).pathname,
            status: r.status(),
            bytes: Buffer.byteLength(body),
            ms: t.responseEnd >= 0 ? t.responseEnd : t.responseStart,
          });
        }
      });
      if (process.env.SEMON_CATALOG_SERVED_UI_OVERRIDE === '1')
        await page.route('**/viewer.js', async (route) =>
          route.fulfill({
            contentType: 'text/javascript',
            body: await readFile(
              join(root, 'crates/semon-sessions/src/viewer.generated.js'),
              'utf8',
            ),
          }),
        );
      const start = performance.now();
      await page.goto(url[1] + '/?t=' + url[2]);
      await page.locator('#page [data-id]').first().waitFor({ timeout: 60000 });
      const firstList = performance.now() - start;
      const key = 'backlog';
      const selectedStart = performance.now();
      await page.locator('#page [data-id="backlog"]').click();
      await page.locator('#page [data-entry-key]').first().waitFor({ timeout: 60000 });
      const selected = performance.now() - selectedStart;
      const preview = page.getByRole('button', { name: 'Load recorded text', exact: true });
      await preview.waitFor();
      const fieldStart = performance.now();
      await preview.click();
      await page.getByRole('button', { name: 'Load more text', exact: true }).waitFor();
      await page.getByRole('button', { name: 'Load more text', exact: true }).click();
      await page.getByText('Complete recorded text loaded.', { exact: true }).waitFor();
      const fieldMs = performance.now() - fieldStart;
      assert.equal(fieldTexts.join(''), scalar);
      assert.equal(
        (await page.locator('#page').innerText()).match(/Recorded source text\./g)?.length,
        5000,
      );
      const first = await page
        .locator('#page [data-entry-key]')
        .first()
        .getAttribute('data-entry-key');
      await page.evaluate(() => document.querySelector('#nav a,#nav button')?.click());
      await page.locator(`#page [data-id="${key}"]`).waitFor();
      const warmStart = performance.now();
      await page.locator(`#page [data-id="${key}"]`).click();
      await page.locator('#page [data-entry-key]').first().waitFor();
      const warm = performance.now() - warmStart;
      assert.equal(
        await page.locator('#page [data-entry-key]').first().getAttribute('data-entry-key'),
        first,
      );
      assert.deepEqual(errors, []);
      assert.equal(
        requests.some((r) => /^\/api\/(model|tool|tx|image)(\?|$)/.test(r)),
        false,
      );
      await page.waitForTimeout(400);
      const evidence = {
        source: process.env.SEMON_CATALOG_SERVED_SOURCE ?? 'unrecorded',
        firstListMs: firstList,
        selectedMs: selected,
        warmSwitchMs: warm,
        fieldMs,
        scalarBytes: Buffer.byteLength(scalar),
        key,
        firstEntry: first,
        requests,
        responses,
        errors,
        body: await page.locator('#page').innerText(),
      };
      if (process.env.SEMON_CATALOG_SERVED_EVIDENCE)
        await writeFile(
          process.env.SEMON_CATALOG_SERVED_EVIDENCE,
          JSON.stringify(evidence, null, 2),
        );
      console.log(JSON.stringify({ ...evidence, body: undefined }, null, 2));
    } finally {
      await browser.close();
      server.kill('SIGKILL');
      await rm(home, { recursive: true, force: true });
    }
  },
);

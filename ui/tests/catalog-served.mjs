import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, writeFile, readFile, rm, readdir, appendFile, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
import { test } from 'node:test';
import { auditText } from '../../tests/ui/text-audit.mjs';
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
    // Source-shaped synthetic Codex desktop records: private bodies replaced.
    // Exercise the native producer, saved recipes and served reader together.
    const nativeAt = new Date(Number(fixture.stdout.trim()) - 1000).toISOString();
    const native = (type, payload) => ({ timestamp: nativeAt, type, payload });
    await writeFile(
      join(home, 'codex/sessions/2026/09/28/rollout-native-tools.jsonl'),
      [
        native('session_meta', {
          id: 'native-tools',
          parent_thread_id: 'h-codex',
          thread_source: 'subagent',
          agent_path: '/root/inspect_tools',
          agent_nickname: 'Socrates',
        }),
        native('response_item', {
          type: 'message',
          role: 'user',
          content: [{ type: 'input_text', text: 'Inspect native tools' }],
        }),
        native('response_item', {
          type: 'reasoning',
          summary: [{ type: 'summary_text', text: 'Before native tools' }],
        }),
        native('response_item', {
          type: 'custom_tool_call',
          name: 'exec',
          call_id: 'wrapper',
          input: "await tools.search({query:'parser'});",
        }),
        native('event_msg', {
          type: 'item_completed',
          item: {
            type: 'McpToolCall',
            id: 'mcp',
            server: 'github',
            tool: 'search',
            arguments: { query: 'parser' },
            status: 'completed',
            result: {
              content: [{ type: 'text', text: 'Tool output stays a tool' }],
              isError: false,
            },
            duration: { secs: 2, nanos: 0 },
          },
        }),
        native('event_msg', {
          type: 'item_completed',
          item: {
            type: 'DynamicToolCall',
            id: 'dynamic',
            namespace: 'functions',
            tool: 'lookup',
            arguments: { key: 'parser' },
            status: 'completed',
            success: false,
            content_items: [{ type: 'input_text', text: 'Lookup failed' }],
          },
        }),
        native('response_item', {
          type: 'custom_tool_call_output',
          call_id: 'wrapper',
          output: [{ type: 'input_text', text: 'Script completed' }],
        }),
        native('response_item', {
          type: 'reasoning',
          summary: [{ type: 'summary_text', text: 'After native tools' }],
        }),
      ]
        .map((record) => JSON.stringify(record))
        .join('\n') + '\n',
    );
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
        fieldTexts = [],
        fieldCaptures = [];
      page.on('pageerror', (e) => errors.push(e.message));
      page.on('request', (r) => {
        if (new URL(r.url()).pathname.startsWith('/api/'))
          requests.push(new URL(r.url()).pathname + new URL(r.url()).search);
      });
      page.on('response', (r) => {
        const capture = (async () => {
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
        })();
        if (new URL(r.url()).pathname === '/api/session-entry' && r.status() === 200)
          fieldCaptures.push(capture);
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
      const query = page.getByRole('textbox', { name: 'Search session details', exact: true });
      await query.fill('backlog');
      await query.press('Enter');
      await page.waitForFunction(
        () => document.querySelectorAll('#page [data-session-row="list"]').length === 1,
      );
      assert.equal(await page.locator('#page [data-id="backlog"]').count(), 1);
      assert.equal(await page.getByRole('button', { name: 'Retry', exact: true }).count(), 0);
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
      await Promise.all(fieldCaptures);
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
      const visual = [];
      for (const width of [390, 1280]) {
        for (const colorScheme of ['light', 'dark']) {
          const context = await browser.newContext({
            viewport: { width, height: 860 },
            colorScheme,
          });
          const check = await context.newPage();
          check.on('pageerror', (e) => errors.push(e.message));
          await check.goto(url[1] + '/?t=' + url[2]);
          await check.locator('#page [data-id="backlog"]').waitFor();
          await check.locator('#page [data-id="backlog"]').click();
          await check.locator('#page [data-entry-key]').first().waitFor();
          await check.evaluate(() => document.fonts.ready);
          const audit = await auditText(check);
          assert.equal(audit.smallCount, 0, JSON.stringify(audit.small));
          assert.equal(audit.lowCount, 0, JSON.stringify(audit.low));
          const geometry = await check.evaluate(() => ({
            overflow: document.documentElement.scrollWidth > innerWidth + 1,
            title: document.querySelector('#topbar')?.textContent,
            entries: document.querySelectorAll('[data-entry-key]').length,
          }));
          assert.equal(geometry.overflow, false, `${width}/${colorScheme} sideways overflow`);
          assert.ok(geometry.title?.trim(), 'Shared toolbar has a readable title');
          assert.ok(
            geometry.entries > 0 && geometry.entries <= 60,
            'Visible transcript is bounded',
          );
          await check.keyboard.press('Tab');
          const focus = await check.evaluate(() => {
            const element = document.activeElement;
            return {
              tag: element?.tagName,
              name: element?.getAttribute('aria-label') || element?.textContent?.trim(),
            };
          });
          assert.notEqual(focus.tag, 'BODY', 'Keyboard reaches a real interactive control');
          assert.ok(focus.name, 'Keyboard control has an accessible name');
          const earlier = check.getByRole('button', { name: 'Load earlier records', exact: true });
          await earlier.waitFor();
          if (width === 390) {
            const box = await earlier.boundingBox();
            assert.ok(
              box.height >= 44 && box.width >= 44,
              'Transcript paging meets phone touch target',
            );
          }
          await earlier.focus();
          await check.keyboard.press('Enter');
          await check.waitForFunction(
            () => document.querySelectorAll('[data-entry-key]').length > 60,
          );
          const screenshotDir = process.env.SEMON_CATALOG_VISUAL_OUT;
          if (screenshotDir) {
            await mkdir(screenshotDir, { recursive: true });
            await check.screenshot({
              path: join(screenshotDir, `catalog-${width}-${colorScheme}.png`),
            });
          }
          visual.push({
            width,
            colorScheme,
            ...geometry,
            focus,
            textAudit: { smallCount: audit.smallCount, lowCount: audit.lowCount },
          });
          await context.close();
        }
      }
      const representative = [];
      for (const session of ['harbor', 'q-codex', 'h-failed', 'native-tools']) {
        const check = await browser.newPage({ viewport: { width: 1280, height: 860 } });
        check.on('pageerror', (e) => errors.push(e.message));
        const nativeRanges = [];
        check.on('response', (response) => {
          if (
            session === 'native-tools' &&
            new URL(response.url()).pathname === '/api/session-transcript' &&
            response.status() === 200
          )
            nativeRanges.push(response.json());
        });
        await check.goto(url[1] + '/?t=' + url[2]);
        const row = check.locator(`#page [data-id="${session}"]`);
        await row.waitFor();
        await row.click();
        await check.locator('#page [data-entry-key]').first().waitFor();
        assert.equal(await check.getByText('Invalid string', { exact: true }).count(), 0);
        assert.equal(await check.getByText('Invalid object', { exact: true }).count(), 0);
        const failed = check.locator('#page .step.err').first();
        if (await failed.count()) {
          await failed.locator(':scope > button').click();
          assert.equal(
            await failed.locator(':scope > button').getAttribute('aria-expanded'),
            'true',
          );
          assert.ok(
            (await failed.innerText()).length > 20,
            'Recorded failure output is inspectable',
          );
        }
        const body = await check.locator('#page').innerText();
        if (session === 'harbor')
          assert.ok(body.includes('Original action prompt is unavailable.'));
        if (session === 'native-tools') {
          const nativeEntries = (await Promise.all(nativeRanges)).flatMap((range) => range.entries);
          const tools = nativeEntries.filter((entry) => entry.k === 'tool');
          assert.deepEqual(
            tools.map((entry) => entry.name),
            ['github.search', 'functions.lookup'],
          );
          assert.deepEqual(
            tools.map((entry) => entry.ok),
            [true, false],
          );
          assert.deepEqual(
            tools.map((entry) => entry.out),
            ['Tool output stays a tool', 'Lookup failed'],
          );
          assert.deepEqual(
            nativeEntries.filter((entry) => entry.k === 'think').map((entry) => entry.text),
            ['Before native tools', 'After native tools'],
          );
          assert.ok(
            (await check.locator('#topbar').innerText()).includes('inspect_tools'),
            'The task path is the session title',
          );
          assert.equal(
            body.includes('Socrates'),
            false,
            'A nickname must not replace the task title',
          );
          assert.equal(await check.locator('#page .step').count(), 2);
        }
        representative.push({ session, readable: true, failedTool: !!(await failed.count()) });
        await check.close();
      }
      assert.deepEqual(errors, []);
      // Losing the observed native source must remain visible without erasing useful
      // previously loaded history or issuing a complete-model restoration request.
      const retainedEntry = await page.locator('#page [data-entry-key]').first().elementHandle();
      await rm(join(home, 'claude', backlog));
      await page.waitForFunction(
        () =>
          /History is incomplete|History is unavailable|Couldn.t read|Retained history remains readable/.test(
            document.querySelector('#page')?.textContent || '',
          ),
        null,
        { timeout: 30000 },
      );
      assert.equal(await retainedEntry.evaluate((node) => node.isConnected), true);
      assert.equal(
        (await page.locator('#page').innerText()).match(/Recorded source text\./g)?.length,
        5000,
      );
      assert.equal(
        requests.some((r) => /^\/api\/(model|tool|tx|image)(\?|$)/.test(r)),
        false,
      );
      await page.waitForTimeout(400);
      const evidence = {
        source: process.env.SEMON_CATALOG_SERVED_SOURCE ?? 'unrecorded',
        visual,
        representative,
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

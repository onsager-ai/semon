import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, writeFile, readFile, rm, readdir, appendFile, mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
import { test } from 'node:test';
import { build } from 'esbuild';
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
    // Deliberately no turn lifecycle: completed tools cannot prove turn completion.
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
      // A supported host can resolve a bookmark while its native Catalog is
      // still publishing. Keep the real authorized response, marking only the
      // first two successful replies incomplete as a slow producer would.
      const bookmark = await browser.newPage();
      const { outputFiles } = await build({
        absWorkingDir: join(root, 'ui'),
        stdin: {
          contents:
            "import {mountViewerApplication} from './src/application'; queueMicrotask(()=>mountViewerApplication({catalogSources:true}));",
          resolveDir: join(root, 'ui'),
        },
        bundle: true,
        write: false,
        format: 'iife',
      });
      let bookmarkReads = 0;
      const bookmarkRequests = [];
      bookmark.on('request', (request) => bookmarkRequests.push(new URL(request.url()).pathname));
      await bookmark.route('**/viewer.js', (route) =>
        route.fulfill({ contentType: 'text/javascript', body: outputFiles[0].text }),
      );
      await bookmark.route('**/api/session-locate?*', async (route) => {
        const response = await route.fetch();
        if (!response.ok()) return route.fulfill({ response });
        const body = await response.json();
        bookmarkReads++;
        return route.fulfill({
          response,
          json: bookmarkReads <= 2 ? { ...body, unavailable: 1 } : body,
        });
      });
      await bookmark.goto(url[1] + '/s/codex/deps?t=' + url[2]);
      await bookmark.getByText('The recorded session index is updating. Retrying…').waitFor();
      await bookmark.waitForFunction(
        () => document.querySelector('#topbar .t')?.textContent === 'deps audit',
        {},
        { timeout: 5000 },
      );
      assert.ok(bookmarkReads >= 3, 'incomplete native lookups retry before selection');
      const bookmarkPath = new URL(bookmark.url()).pathname;
      assert.equal(bookmarkPath, '/session/local/deps');
      assert.deepEqual(bookmarkPath.split('/').filter(Boolean), ['session', 'local', 'deps']);
      const sourceLabel = (await readFile(join(home, 'proc/sys/kernel/hostname'), 'utf8')).trim();
      await bookmark.waitForFunction(
        (label) =>
          document.querySelector('#page .divider.started .dv-machine')?.textContent === label,
        sourceLabel,
        { timeout: 5000 },
      );
      assert.ok(
        bookmarkRequests.every((path) => !/^\/api\/(model|tx|events)(?:\/|$)/.test(path)),
        'bookmark resolution does not warm a workspace model',
      );
      await bookmark.close();
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
      await page.goto(url[1] + '/sessions?t=' + url[2]);
      await page.locator('#page [data-id]').first().waitFor({ timeout: 60000 });
      const firstList = performance.now() - start;
      const query = page.getByRole('searchbox', { name: 'Search session details', exact: true });
      const beforeTyping = requests.length;
      await query.fill('backlog');
      assert.equal(
        await page.evaluate(() => new URL(location.href).searchParams.get('q')),
        'backlog',
      );
      await page.evaluate(
        () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
      );
      assert.ok(
        requests.slice(beforeTyping).every((path) => {
          const target = new URL(path, url[1]);
          return target.pathname !== '/api/sessions' || target.searchParams.get('q') !== 'backlog';
        }),
        'typing a draft does not submit native metadata reads',
      );
      await query.press('Enter');
      await page.waitForFunction(
        () => document.querySelectorAll('#page [data-session-row="list"]').length === 1,
      );
      assert.equal(await page.locator('#page [data-id="backlog"]').count(), 1);
      assert.equal(await page.getByRole('button', { name: 'Retry', exact: true }).count(), 0);
      const key = 'backlog';
      const selectedStart = performance.now();
      await page.locator('#page [data-id="backlog"]').click();
      await page.locator('#page [data-entry-key]:visible').first().waitFor({ timeout: 60000 });
      await page.waitForFunction(
        () => document.activeElement === document.querySelector('#page .ph > h1'),
      );
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
      await page.evaluate(() => document.querySelector('#nav [data-go="sessions"]')?.click());
      await page.locator(`#page [data-id="${key}"]`).waitFor();
      const warmStart = performance.now();
      await page.locator(`#page [data-id="${key}"]`).click();
      await page.locator('#page [data-entry-key]').first().waitFor({ state: 'attached' });
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
      for (const width of [390, 820, 1280]) {
        for (const colorScheme of ['light', 'dark']) {
          const context = await browser.newContext({
            viewport: { width, height: 860 },
            colorScheme,
          });
          const check = await context.newPage();
          check.on('pageerror', (e) => errors.push(e.message));
          await check.goto(url[1] + '/sessions?t=' + url[2]);
          await check.locator('#page [data-id="backlog"]').waitFor();
          await check.locator('#page [data-id="backlog"]').click();
          await check.locator('#page [data-entry-key]:visible').first().waitFor();
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
      const nativeDeadline = Date.now() + 30_000;
      const nativeContext = await browser.newContext({ javaScriptEnabled: false });
      const nativeNavigationDiagnostics = [];
      try {
        const nativePage = await nativeContext.newPage();
        const nativeRequests = [];
        const recordsByRequest = new Map();
        let activeNavigationLabel = 'native navigation';
        const remainingNativeMs = () => Math.max(1, nativeDeadline - Date.now());
        nativePage.on('request', (request) => {
          const requestURL = new URL(request.url());
          nativeRequests.push(requestURL.pathname);
          if (!request.isNavigationRequest()) return;
          const record = {
            label: activeNavigationLabel,
            method: request.method(),
            requestPath: requestURL.pathname,
            status: null,
            canonicalPath: null,
            dom: '',
          };
          nativeNavigationDiagnostics.push(record);
          recordsByRequest.set(request, record);
        });
        nativePage.on('response', (response) => {
          const record = recordsByRequest.get(response.request());
          if (record) record.status = response.status();
        });
        const finishNativeDiagnostic = async (start, label) => {
          const records = nativeNavigationDiagnostics.slice(start);
          let dom = '';
          try {
            dom = await nativePage
              .locator('body')
              .innerText({ timeout: Math.min(500, remainingNativeMs()) });
          } catch {
            dom = '(body text unavailable)';
          }
          dom = dom.split(url[2]).join('[redacted bootstrap token]').slice(0, 240);
          const currentURL = new URL(nativePage.url());
          if (!records.length) {
            const record = {
              label,
              method: null,
              requestPath: null,
              status: null,
              canonicalPath: currentURL.pathname,
              dom,
            };
            nativeNavigationDiagnostics.push(record);
            return [record];
          }
          for (const record of records) {
            record.canonicalPath = currentURL.pathname;
            record.dom = dom;
          }
          return records;
        };
        const captureNativeNavigation = async (label, action) => {
          const start = nativeNavigationDiagnostics.length;
          activeNavigationLabel = label;
          try {
            await action(remainingNativeMs());
          } catch (error) {
            // Snapshot the failure page before the outer finally closes the context.
            await finishNativeDiagnostic(start, label);
            throw error;
          }
          const records = await finishNativeDiagnostic(start, label);
          return records.at(-1);
        };
        const navigateNativeWithRetry = async (
          label,
          action,
          canonicalPath,
          requestedPath = canonicalPath,
        ) => {
          let record = await captureNativeNavigation(label, action);
          let detail = JSON.stringify(record);
          assert.equal(record.method, 'GET', `${label} navigation is a read: ${detail}`);
          assert.ok(
            record.status === 200 || record.status === 503,
            `${label} unexpected HTTP status: ${detail}`,
          );
          if (record.status === 503) {
            assert.equal(
              record.canonicalPath,
              requestedPath,
              `${label} retry starts from the requested path: ${detail}`,
            );
            const retryMessage =
              'The recorded session index is updating. Reload this page to retry.';
            assert.equal(
              await nativePage.getByText(retryMessage, { exact: true }).count(),
              1,
              `${label} 503 retry message: ${detail}`,
            );
            const href = await nativePage
              .getByRole('link', { name: 'Reload to retry', exact: true })
              .getAttribute('href', { timeout: remainingNativeMs() });
            assert.ok(href, `${label} 503 retry link is missing: ${detail}`);
            const retryURL = new URL(href, url[1]);
            const currentURL = new URL(nativePage.url());
            assert.equal(retryURL.origin, currentURL.origin, `${label} retry stays same-origin`);
            assert.ok(
              [requestedPath, canonicalPath].includes(retryURL.pathname),
              `${label} retry keeps its requested or canonical path`,
            );
            assert.equal(
              retryURL.searchParams.get('native'),
              '1',
              `${label} retry keeps native mode`,
            );
            assert.equal(
              retryURL.searchParams.has('t'),
              false,
              `${label} retry omits the bootstrap secret`,
            );
            for (const name of ['machine', 'generation']) {
              if (currentURL.searchParams.has(name)) {
                assert.equal(
                  retryURL.searchParams.get(name),
                  currentURL.searchParams.get(name),
                  `${label} retry keeps ${name}`,
                );
              }
            }
            record = await captureNativeNavigation(`${label} documented GET retry`, (timeout) =>
              nativePage.goto(retryURL.href, { timeout }),
            );
            detail = JSON.stringify(record);
            assert.equal(record.method, 'GET', `${label} documented retry is a read`);
            assert.equal(
              record.canonicalPath,
              canonicalPath,
              `${label} GET retry canonical path: ${JSON.stringify(record)}`,
            );
          }
          if (record.status === 200) {
            assert.equal(record.canonicalPath, canonicalPath, `${label} canonical path: ${detail}`);
          }
          assert.equal(
            record.status,
            200,
            `${label} needs a served page: ${JSON.stringify(record)}`,
          );
          return record;
        };

        try {
          const listing = await captureNativeNavigation('native Sessions listing', (timeout) =>
            nativePage.goto(url[1] + '/sessions?native=1&t=' + url[2], { timeout }),
          );
          assert.equal(listing.canonicalPath, '/sessions', `listing: ${JSON.stringify(listing)}`);
          assert.equal(listing.status, 200, `listing: ${JSON.stringify(listing)}`);
          await navigateNativeWithRetry(
            'native backlog selection',
            (timeout) =>
              nativePage.locator('a[href^="/session/local/backlog?"]').click({ timeout }),
            '/session/local/backlog',
          );
          await nativePage
            .getByRole('region', { name: 'Session transcript' })
            .waitFor({ timeout: remainingNativeMs() });
          const earlier = nativePage.getByRole('link', { name: 'Earlier records', exact: true });
          const anchor = new URL(
            await earlier.getAttribute('href', { timeout: remainingNativeMs() }),
            url[1],
          );
          assert.ok(
            anchor.searchParams.get('generation'),
            'HTML paging binds its native generation',
          );
          assert.equal(anchor.pathname, '/session/local/backlog');
          assert.equal(anchor.searchParams.has('machine'), false);
          await navigateNativeWithRetry(
            'native earlier records',
            (timeout) => earlier.click({ timeout }),
            '/session/local/backlog',
          );
          assert.equal(
            new URL(nativePage.url()).searchParams.get('generation'),
            anchor.searchParams.get('generation'),
          );
          assert.ok(
            (await nativePage
              .locator('section[aria-label="Session transcript"] > article')
              .count()) <= 60,
          );
          await navigateNativeWithRetry(
            'native tools selection',
            (timeout) =>
              nativePage.goto(url[1] + '/s/codex/native-tools?machine=local&native=1', { timeout }),
            '/session/local/native-tools',
            '/s/codex/native-tools',
          );
          const thinking = nativePage
            .locator('details')
            .filter({ has: nativePage.locator('summary', { hasText: /^Thinking$/ }) });
          assert.ok(
            (await thinking.count()) > 0,
            'native reasoning remains accessible without JavaScript',
          );
          assert.equal(
            await thinking.first().getAttribute('open'),
            null,
            'Thinking starts collapsed',
          );
          assert.ok(
            nativeRequests.every(
              (path) => !/^\/api\/(model|tx|events|analytics)(?:\/|$)/.test(path),
            ),
          );
          console.log(
            `Native served navigation diagnostics: ${JSON.stringify(nativeNavigationDiagnostics)}`,
          );
        } catch (error) {
          const message = String(error?.stack || error)
            .split(url[2])
            .join('[redacted bootstrap token]');
          const diagnostics = JSON.stringify(nativeNavigationDiagnostics);
          console.error(
            `Native served navigation diagnostics before cleanup: ${diagnostics}\n${message}`,
          );
          throw new Error(message);
        }
      } finally {
        await nativeContext.close();
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
        await check.goto(url[1] + '/sessions?t=' + url[2]);
        const row = check.locator(`#page [data-id="${session}"]`);
        await row.waitFor();
        await row.click();
        await check.locator('#page [data-entry-key]').first().waitFor({ state: 'attached' });
        assert.equal(await check.getByText('Invalid string', { exact: true }).count(), 0);
        assert.equal(await check.getByText('Invalid object', { exact: true }).count(), 0);
        const failed = check.locator('#page .step.err').first();
        if (await failed.count()) {
          // Native tools retain the shared collapsed event-group presentation.
          await failed.evaluate((node) =>
            node
              .closest('.tgroup')
              ?.querySelector(':scope > button[aria-expanded="false"]')
              ?.click(),
          );
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
        if (session === 'harbor') {
          assert.equal(body.includes('Original action prompt is unavailable.'), false);
          assert.ok(
            await check.locator('#page .child-card').count(),
            'recorded child sessions are presented',
          );
          assert.ok(
            await check.locator('#page [data-turn]').count(),
            'recorded turn navigation is retained',
          );
        }
        if (session === 'native-tools') {
          const ranges = await Promise.all(nativeRanges);
          assert.ok(ranges.length);
          for (const range of ranges) {
            assert.equal(range.session.details.state, 'off');
            assert.equal(range.session.details.turn_lifecycle, null);
          }
          const nativeEntries = ranges.flatMap((range) => range.entries);
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
      await page.bringToFront();
      await page.waitForFunction(() => document.visibilityState === 'visible');
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

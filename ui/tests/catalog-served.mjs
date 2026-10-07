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
    const searchFixture = process.env.SEMON_CATALOG_SERVED_REQUIRE_SEARCH === '1';
    if (searchFixture) {
      const project = join(home, 'claude', 'projects', 'metadata-search');
      await mkdir(project, { recursive: true });
      for (let index = 0; index <= 600; index++) {
        const id = index === 600 ? 'metadata-needle' : `metadata-decoy-${index}`;
        await writeFile(
          join(project, id + '.jsonl'),
          JSON.stringify({
            type: 'user',
            sessionId: id,
            uuid: id + '-user',
            timestamp: new Date(
              Number(fixture.stdout.trim()) - (index === 600 ? 3 : 2) * 86400000,
            ).toISOString(),
            message: {
              role: 'user',
              content: index === 600 ? 'abcXYZabc' : 'abc abcX bcXY cXYZ XYZa YZab Zabc',
            },
          }) + '\n',
        );
      }
    }
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
      const firstRows = performance.now() - start;
      await page.locator('#page [data-id="backlog"]').waitFor({ timeout: 60000 });
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
      const metadataSearch = [];
      await page.evaluate(() => document.querySelector('#nav a,#nav button')?.click());
      await page.locator('#page [data-id="backlog"]').waitFor();
      const search = page.getByRole('textbox', { name: 'Search session details', exact: true });
      if (process.env.SEMON_CATALOG_SERVED_REQUIRE_SEARCH === '1') await search.waitFor();
      if (await search.count()) {
        if (searchFixture) {
          // A complete source index makes this a candidate-budget continuation,
          // rather than the separately visible background-discovery condition.
          await page.waitForFunction(
            () =>
              !document
                .querySelector('#page')
                ?.textContent?.includes('More sessions are being discovered.'),
            null,
            { timeout: 60000 },
          );
          const firstSearch = page.waitForResponse((response) => {
            const requested = new URL(response.url());
            return (
              requested.pathname === '/api/sessions' &&
              requested.searchParams.get('q') === 'abcXYZabc' &&
              !requested.searchParams.has('cursor') &&
              response.status() === 200
            );
          });
          await search.fill('abcXYZabc');
          await search.press('Tab');
          const firstPage = await (await firstSearch).json();
          assert.equal(firstPage.search.index_complete, true);
          assert.equal(firstPage.search.partial, true);
          assert.equal(firstPage.search.candidates, 512);
          assert.deepEqual(firstPage.items, []);
          assert.ok(firstPage.next_cursor);
          await page
            .getByText(
              'Search checked a bounded part of the index. Load more sessions to continue looking for matches.',
            )
            .waitFor();
          assert.equal(
            await page
              .getByText('No recorded sessions match these filters.', { exact: true })
              .count(),
            0,
          );
          const more = page.getByRole('button', { name: 'Load more sessions', exact: true });
          await more.focus();
          await page.keyboard.press('Enter');
          await page.locator('#page [data-id="metadata-needle"]').waitFor();
          assert.equal(await search.inputValue(), 'abcXYZabc');
          metadataSearch.push({ query: 'abcXYZabc', partialContinuation: true, candidates: 512 });
        }
        for (const [query, expected] of [
          ['backlog', true],
          ['Actual native scalar field.', false],
          ['', true],
        ]) {
          const started = performance.now();
          const response = page.waitForResponse((response) => {
            const requested = new URL(response.url());
            return (
              requested.pathname === '/api/sessions' &&
              requested.searchParams.get('q') === (query || null) &&
              response.status() === 200
            );
          });
          await search.fill(query);
          await search.press('Tab');
          const body = await (await response).json();
          if (expected) await page.locator('#page [data-id="backlog"]').waitFor();
          else {
            await page
              .getByText('No recorded sessions match these filters.', { exact: true })
              .waitFor();
            assert.equal(await page.locator('#page [data-id]').count(), 0);
          }
          assert.equal(await search.inputValue(), query);
          assert.equal(
            body.search?.semantics ?? null,
            query ? 'unicode_lowercase_substring' : null,
          );
          metadataSearch.push({ query, ms: performance.now() - started, rows: body.items.length });
        }
      }
      await page.locator('#page [data-id="backlog"]').click();
      await page.getByText('Complete recorded text loaded.', { exact: true }).waitFor();
      assert.equal(
        (await page.locator('#page').innerText()).match(/Recorded source text\./g)?.length,
        5000,
      );
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
        metadataSearch,
        firstListMs: firstList,
        firstRowsMs: firstRows,
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

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';

const root = new URL('../../', import.meta.url).pathname;
test(
  'served canonical Home, Analytics and contextual Composer across layouts and themes',
  {
    skip: !process.env.SEMON_CATALOG_SERVED_BIN,
  },
  async () => {
    const scratch = await mkdtemp(join(tmpdir(), 'semon-overview-'));
    const fixture = spawnSync(process.execPath, [join(root, 'tests/ui/fixture.mjs'), scratch], {
      encoding: 'utf8',
    });
    assert.equal(fixture.status, 0, fixture.stderr);
    const server = spawn(
      process.env.SEMON_CATALOG_SERVED_BIN,
      [
        'sessions',
        '--serve',
        '--listen',
        '127.0.0.1:0',
        '--claude-home',
        join(scratch, 'claude'),
        '--claude-json',
        join(scratch, '.claude.json'),
        '--codex-home',
        join(scratch, 'codex'),
        '--copilot-home',
        join(scratch, 'copilot'),
        '--proc-root',
        join(scratch, 'proc'),
        '--cache',
        join(scratch, 'index.json'),
      ],
      {
        env: { ...process.env, SEMON_TEST_NOW: fixture.stdout.trim(), TZ: 'UTC' },
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    );
    let output = '';
    for (const stream of [server.stdout, server.stderr])
      stream.on('data', (data) => (output += data));
    const browser = await chromium.launch();
    const captures = process.env.SEMON_OVERVIEW_VISUAL_OUT;
    const evidence = [];
    try {
      let address;
      for (let attempt = 0; attempt < 200; attempt++) {
        address = output.match(/(http:\/\/127\.0\.0\.1:\d+)\/\?t=([a-f0-9]+)/);
        if (address) break;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      assert.ok(address, output);
      const landing = await fetch(address[1] + '/?t=' + address[2]);
      const headers = {
        cookie: landing.headers
          .getSetCookie()
          .map((value) => value.split(';')[0])
          .join('; '),
      };
      let home;
      for (let attempt = 0; attempt < 120; attempt++) {
        const response = await fetch(address[1] + '/api/viewer/home', { headers });
        home = await response.json();
        if (home.observations?.length === 0 && home.inbox?.length) break;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.deepEqual(home.observations, [], 'Home source publication completes');
      assert.ok(home.inbox.length, 'fixture has needs-you activity');
      // Standalone model remains a legitimate local oracle. Browser requests
      // below must use only the canonical transport.
      const model = await (await fetch(address[1] + '/api/model', { headers })).json();
      const question = model.handoffs.find(
        (event) => event.ask === 'question' && event.status === 'wait',
      );
      assert.ok(question?.brief);
      if (captures) await mkdir(captures, { recursive: true });
      for (const width of [390, 820, 1280])
        for (const colorScheme of ['light', 'dark']) {
          const context = await browser.newContext({
            viewport: { width, height: 860 },
            colorScheme,
            timezoneId: 'UTC',
          });
          try {
            const page = await context.newPage();
            page.setDefaultTimeout(60000);
            await page.clock.setFixedTime(Number(fixture.stdout.trim()));
            const requests = [],
              errors = [];
            page.on('request', (request) => {
              const url = new URL(request.url());
              if (url.pathname.startsWith('/api/')) requests.push(url.pathname);
            });
            page.on('pageerror', (error) => errors.push(error.message));
            const screenshot = async (name) => {
              await page.evaluate(() => document.fonts.ready);
              await page.evaluate(() =>
                Promise.all(
                  document
                    .getAnimations()
                    .filter(
                      (animation) => animation.effect?.getComputedTiming().iterations !== Infinity,
                    )
                    .map((animation) => animation.finished.catch(() => {})),
                ),
              );
              if (captures)
                await page.screenshot({
                  path: join(captures, `${name}-${width}-${colorScheme}.png`),
                  fullPage: name !== 'composer',
                });
            };
            const navigate = async (destination) => {
              if (width === 390 && (await page.locator('#topbar .trace-back').count())) {
                await page.locator('#topbar .trace-back').click();
                await page.locator('.session-composer-trigger').waitFor();
              }
              if (width === 390)
                await page.getByRole('button', { name: 'Open menu', exact: true }).click();
              await page.locator(`#nav a[data-go="${destination}"]`).click();
            };
            const delayed = width === 390 && colorScheme === 'light';
            let holdPreviews = delayed;
            const held = [];
            if (delayed)
              await page.route('**/api/session-home?**', async (route) => {
                const response = await route.fetch();
                if (holdPreviews) await new Promise((resolve) => held.push(resolve));
                await route.fulfill({ response }).catch(() => {});
              });
            await page.goto(address[1] + '/?t=' + address[2] + '&compat=1');
            await page.locator('#page h1').filter({ hasText: 'Home' }).waitFor();
            await page.locator('#page .ib').first().waitFor();
            await page.locator('#lanes .srow').first().waitFor({ state: 'attached' });
            if (delayed) {
              assert.equal(
                await page.locator('#page .ib:not(.quiet)').count(),
                home.inbox.length,
                'activity renders while native previews are pending',
              );
              assert.equal(await page.getByText(question.brief, { exact: true }).count(), 0);
              await navigate('analytics');
              await page.locator('#page h1').filter({ hasText: 'Analytics' }).waitFor();
              holdPreviews = false;
              for (const release of held) release();
              await page.locator('.analytics-metrics').waitFor();
              assert.equal(
                await page.locator('#page .ib').count(),
                0,
                'late preview cannot replace Analytics',
              );
              await navigate('home');
              await page.locator('#page h1').filter({ hasText: 'Home' }).waitFor();
            }
            await page.getByText(question.brief, { exact: true }).waitFor();
            assert.equal(await page.locator('#page .ib:not(.quiet)').count(), home.inbox.length);
            assert.ok(
              (await page.locator('#page .ib .q').allTextContents()).every(
                (text) => text.trim().length,
              ),
            );
            assert.ok((await page.locator('#lanes > .treeitem').count()) <= 8);
            assert.ok(
              (await page.locator('#lanes .tree-group .treeitem').count()) > 0,
              'resolved ancestry remains nested',
            );
            assert.equal(new URL(page.url()).searchParams.has('compat'), false);
            await screenshot('home');
            await page.locator('#page .tracebtn').first().click();
            await page.locator('#page h1').filter({ hasText: 'Trace' }).waitFor();
            await page.getByRole('region', { name: 'Agents', exact: true }).waitFor();
            assert.match(new URL(page.url()).pathname, /^\/trace\//);
            assert.ok(
              (await page.locator('#page [data-agent-id]').count()) > 1,
              'causal agents are connected',
            );
            await page.getByText(question.brief, { exact: true }).waitFor();
            assert.ok(
              (await page.locator('#page .brief.md').count()) > 0,
              'native causal text is visible',
            );
            await screenshot('trace');
            await navigate('analytics');
            await page.locator('#page h1').filter({ hasText: 'Analytics' }).waitFor();
            await page.locator('.analytics-metrics').waitFor();
            await screenshot('analytics');
            await navigate('machines');
            await page.locator('#page [data-m]').first().click();
            await page.getByRole('button', { name: 'Back to Machines', exact: true }).waitFor();
            await page.locator('#page .nrow[data-id]').first().waitFor();
            assert.equal(await page.locator('#page.machine-page').count(), 1);
            assert.match(new URL(page.url()).pathname, /^\/machines\//);
            await page.reload();
            await page.getByRole('button', { name: 'Back to Machines', exact: true }).waitFor();
            assert.match(
              await page.locator('#topbar .meta-line').textContent(),
              /Up.*working.*sessions?/,
            );
            assert.equal(
              await page.locator('#topbar .ttl > .l1').count(),
              1,
              'machine header has one owner',
            );
            await page.waitForFunction(
              () => document.querySelector('#page')?.getAttribute('aria-busy') !== 'true',
            );
            assert.match(
              await page.locator('#page .nrow').filter({ hasText: 'Principal' }).textContent(),
              /Morning sweep/,
              'machine prompt preview uses selected native evidence',
            );
            const nativeMachine = await page.request.get(page.url() + '&native=1');
            assert.equal(nativeMachine.status(), 200, 'native machine bookmark reloads');
            assert.match(await nativeMachine.text(), /Principal.*All sessions/s);
            await screenshot('machine');
            await page.getByRole('button', { name: 'Back to Machines', exact: true }).click();
            await page.locator('#page h1').filter({ hasText: 'Machines' }).waitFor();
            assert.equal(
              await page.locator('#page.machine-page').count(),
              0,
              'leaving machine detail releases its screen geometry',
            );
            await navigate('sessions');
            await page.locator('#page [data-session-row="list"][data-id="harbor"]').click();
            await page.locator('[aria-label="Transcript"]').waitFor();
            await page.locator('#page .child-card').first().waitFor();
            assert.equal(
              await page
                .getByText('Original action prompt is unavailable.', { exact: true })
                .count(),
              0,
            );
            assert.ok(await page.locator('#page [data-turn]').count());
            assert.equal(
              await page.locator('#page [aria-label="Conversation controls"]').count(),
              0,
            );
            const trigger = page.getByRole('button', { name: 'Open Composer', exact: true });
            assert.equal(await trigger.getAttribute('aria-expanded'), 'false');
            await trigger.click();
            const panel = page.getByRole('dialog', { name: 'Session Composer', exact: true });
            await panel.waitFor();
            assert.equal(await trigger.getAttribute('aria-expanded'), 'true');
            await screenshot('composer');
            await page.keyboard.press('Escape');
            assert.equal(await panel.count(), 0);
            assert.equal(await trigger.evaluate((node) => node === document.activeElement), true);
            await page.waitForFunction(() => !history.state?.sheet);
            const sessionUrl = page.url();
            await trigger.click();
            assert.equal(await page.evaluate(() => history.state?.sheet), 1);
            await page.goBack();
            await panel.waitFor({ state: 'detached' });
            assert.equal(page.url(), sessionUrl, 'Back closes Composer on the same session');
            assert.equal(await trigger.evaluate((node) => node === document.activeElement), true);
            assert.equal(
              await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1),
              false,
            );
            assert.equal(
              requests.some((path) => /^\/api\/(model|tx|analytics)(\/|$)/.test(path)),
              false,
            );
            assert.deepEqual(errors, []);
            evidence.push({ width, colorScheme, requests, errors });
          } finally {
            await context.close();
          }
        }
      if (captures)
        await writeFile(join(captures, 'evidence.json'), JSON.stringify(evidence, null, 2) + '\n');
    } finally {
      await browser.close();
      server.kill();
      await new Promise((resolve) => {
        if (server.exitCode !== null) resolve();
        else server.once('exit', resolve);
      });
      await rm(scratch, { recursive: true, force: true });
    }
  },
);

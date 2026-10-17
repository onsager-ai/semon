import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';

const root = new URL('../../', import.meta.url).pathname;
test(
  'served collection Home, Analytics and selected session across layouts and themes',
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
      // The real collection server rejects stale delivery before parsing its
      // payload; no controller or synthetic product bypass is supplied here.
      for (const body of [
        JSON.stringify({ op: 'send', id: 'harbor', text: 'synthetic refused input' }),
        'malformed control fixture request',
      ]) {
        const refused = await fetch(address[1] + '/api/control', {
          method: 'POST',
          headers: { ...headers, origin: address[1], 'content-type': 'application/json' },
          body,
        });
        assert.equal(refused.status, 503, 'authenticated stale control is unavailable');
        assert.equal((await refused.json()).error.code, 'capability_unavailable');
      }
      let home;
      for (let attempt = 0; attempt < 120; attempt++) {
        const response = await fetch(address[1] + '/api/viewer/home', { headers });
        home = await response.json();
        if (home.observations?.length === 0 && home.inbox?.length) break;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.deepEqual(home.observations, [], 'Home source publication completes');
      assert.ok(home.inbox.length, 'fixture has needs-you activity');
      const waiting = home.inbox.find(
        (row) => row.event?.ask === 'question' && row.event.status === 'wait',
      );
      assert.ok(waiting, 'fixture has a source-qualified waiting question');
      const query = new URLSearchParams({
        machine: waiting.source_key,
        sid: waiting.catalog_key,
        scope: 'retained_history',
        generation: waiting.session.content_generation,
        events: JSON.stringify([waiting.event.id]),
      });
      const previewResponse = await fetch(address[1] + '/api/session-home?' + query, { headers });
      assert.equal(previewResponse.status, 200);
      const preview = await previewResponse.json();
      assert.equal(preview.identity.source_key, waiting.source_key);
      assert.equal(preview.identity.catalog_key, waiting.catalog_key);
      assert.equal(preview.generation, waiting.session.content_generation);
      const question = preview.events[waiting.event.id];
      assert.ok(question?.brief);
      if (captures) await mkdir(captures, { recursive: true });
      for (const width of [390, 820, 1280])
        for (const colorScheme of ['light', 'dark']) {
          const context = await browser.newContext({
            viewport: { width, height: 860 },
            colorScheme,
            timezoneId: 'UTC',
          });
          let captureServedFixtureFailure;
          try {
            const page = await context.newPage();
            page.setDefaultTimeout(60000);
            await page.clock.setFixedTime(Number(fixture.stdout.trim()));
            const requests = [],
              errors = [],
              apiResponses = [];
            page.on('request', (request) => {
              const url = new URL(request.url());
              if (url.pathname.startsWith('/api/')) requests.push(url.pathname);
            });
            page.on('response', (response) => {
              const url = new URL(response.url());
              if (!url.pathname.startsWith('/api/')) return;
              apiResponses.push({
                path: url.pathname,
                status: response.status(),
                machine: url.searchParams.get('machine'),
                sid: url.searchParams.get('sid'),
                scope: url.searchParams.get('scope'),
                response,
              });
              if (apiResponses.length > 32) apiResponses.shift();
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
                  fullPage: name !== 'session-menu',
                });
            };
            captureServedFixtureFailure = async (stage, context = {}) => {
              const ui = await page.evaluate(() => {
                const text = (node, limit = 500) =>
                  (node?.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, limit);
                const buttons = [
                  ...document.querySelectorAll('#page button, #page [role="button"]'),
                ]
                  .slice(0, 48)
                  .map((node) => ({
                    tag: node.tagName.toLowerCase(),
                    text: text(node, 140),
                    ariaLabel: node.getAttribute('aria-label'),
                    title: node.getAttribute('title'),
                    disabled: node.matches(':disabled'),
                    hidden: node.hidden || getComputedStyle(node).display === 'none',
                  }));
                const recent = [...document.querySelectorAll('#lanes .treeitem')]
                  .slice(0, 32)
                  .map((node) => {
                    const id = node.getAttribute('data-id') ?? '',
                      group = node.querySelector(':scope > .tree-group'),
                      children = group
                        ? [...group.querySelectorAll(':scope > .treeitem')].slice(0, 8)
                        : [];
                    return {
                      id,
                      text: text(node, 100),
                      expanded:
                        node.getAttribute('aria-expanded') ??
                        node.querySelector('[aria-expanded]')?.getAttribute('aria-expanded') ??
                        null,
                      groupHidden: group ? group.hidden : null,
                      ...(id.includes('atlas-ingest')
                        ? {
                            childCount: group?.querySelectorAll(':scope > .treeitem').length ?? 0,
                            childIds: children.map((child) => child.getAttribute('data-id') ?? ''),
                          }
                        : {}),
                    };
                  });
                const cards = [...document.querySelectorAll('#page .child-card')]
                  .slice(0, 12)
                  .map((node) => ({
                    tag: node.tagName.toLowerCase(),
                    className: node.getAttribute('class'),
                    id: node.getAttribute('data-id'),
                    text: text(node, 160),
                  }));
                const foot = document.querySelector('#page .session-foot');
                return {
                  path: location.pathname,
                  route: history.state?.v ?? null,
                  readyState: document.readyState,
                  visibilityState: document.visibilityState,
                  title: document.title.slice(0, 160),
                  heading: text(document.querySelector('#page h1'), 200),
                  topbar: text(document.querySelector('#topbar'), 500),
                  footer: text(foot, 1000),
                  footerClass: foot?.getAttribute('class') ?? null,
                  pageText: text(document.querySelector('#page'), 6000),
                  buttons,
                  recent,
                  cards,
                };
              });
              const safeFields = new Set([
                'identity',
                'session',
                'key',
                'id',
                'name',
                'machine',
                'source_key',
                'sourceKey',
                'catalog_key',
                'catalogKey',
                'read_scope',
                'readScope',
                'generation',
                'content_generation',
                'contentGeneration',
                'errors_generation',
                'errorsGeneration',
                'status',
                'state',
                'kind',
                'ask',
                'source_turn',
                'sourceTurn',
                'parent_turn',
                'parentTurn',
                'parent_session',
                'source_session',
                'session_key',
                'session_id',
                'role',
                'live_observation',
                'liveObservation',
                'details',
                'family',
                'origin',
                'edge',
                'edges',
                'from',
                'to',
                'parent_name',
                'parentName',
                'relationship_context',
                'context',
                'sessions',
                'handoffs',
                'ancestors',
                'runs',
                'selected',
                'parent',
                'items',
                'recent',
                'inbox',
                'working',
                'answered',
                'events',
                'entries',
                'turns',
                'children',
                'relationships',
              ]);
              const summarize = (value, depth = 0) => {
                if (depth > 3) return '[depth limit]';
                if (value === null || ['string', 'number', 'boolean'].includes(typeof value))
                  return typeof value === 'string' ? value.slice(0, 100) : value;
                if (Array.isArray(value))
                  return {
                    length: value.length,
                    sample: value.slice(0, 8).map((item) => summarize(item, depth + 1)),
                  };
                if (typeof value !== 'object') return typeof value;
                const fields = {};
                for (const [key, item] of Object.entries(value)) {
                  if (!safeFields.has(key)) continue;
                  if (
                    [
                      'events',
                      'entries',
                      'turns',
                      'children',
                      'relationships',
                      'edges',
                      'sessions',
                      'handoffs',
                      'ancestors',
                      'runs',
                      'items',
                      'recent',
                      'inbox',
                      'working',
                      'answered',
                    ].includes(key)
                  ) {
                    const values = Array.isArray(item)
                      ? item.slice(0, 8).map((entry) => summarize(entry, depth + 1))
                      : item && typeof item === 'object'
                        ? Object.entries(item)
                            .slice(0, 8)
                            .map(([id, entry]) => ({
                              id: id.slice(0, 80),
                              value: summarize(entry, depth + 1),
                            }))
                        : summarize(item, depth + 1);
                    fields[key] =
                      Array.isArray(item) || (item && typeof item === 'object')
                        ? {
                            length: Array.isArray(item) ? item.length : Object.keys(item).length,
                            sample: values,
                          }
                        : values;
                  } else fields[key] = summarize(item, depth + 1);
                }
                return { keys: Object.keys(value).slice(0, 40), fields };
              };
              const relevant = apiResponses
                .filter((item) =>
                  [
                    '/api/viewer/home',
                    '/api/sessions',
                    '/api/session-home',
                    '/api/session-transcript',
                    '/api/session-identity',
                  ].includes(item.path),
                )
                .slice(-8);
              let timer;
              const api = await Promise.race([
                Promise.all(
                  relevant.map(async ({ response, ...item }) => {
                    try {
                      return { ...item, metadata: summarize(await response.json()) };
                    } catch {
                      return { ...item, metadata: '[body unavailable]' };
                    }
                  }),
                ),
                new Promise((resolve) => {
                  timer = setTimeout(() => resolve('[metadata deadline]'), 500);
                }),
              ]);
              clearTimeout(timer);
              console.error(
                `[served fixture diagnostic ${width} ${colorScheme}] ${JSON.stringify({
                  stage,
                  context,
                  ui,
                  api,
                  pageErrors: errors.slice(-12),
                })}`,
              );
            };
            const navigate = async (destination) => {
              if (width === 390 && (await page.locator('#topbar .trace-back').count())) {
                await page.locator('#topbar .trace-back').click();
                await page
                  .getByRole('button', {
                    name: 'Session menu: details, cost and actions',
                    exact: true,
                  })
                  .waitFor();
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
            // The sample's six direct ingest children include one nested child.
            // The published All action opens all seven recorded descendants.
            await page
              .locator('[aria-label="All 7 sessions under atlas ingest"]')
              .waitFor({ state: 'attached' });
            assert.equal(
              await page.locator('[aria-label="All 7 sessions under atlas ingest"]').count(),
              1,
            );
            await page.locator('#page .ib').first().waitFor();
            await page.locator('#lanes .srow').first().waitFor({ state: 'attached' });
            assert.equal(
              await page.locator('#lanes .child-dot').count(),
              0,
              'expanded Recent keeps descendant status in its flag, without a rail dot shifting age',
            );
            if (width === 1280 && colorScheme === 'light') {
              // Leave selected-row discovery to transcript metadata, not Home or page summaries.
              const withoutHomeSelected = async (route) => {
                const response = await route.fetch();
                const body = await response.json();
                for (const field of ['recent', 'inbox', 'working', 'answered'])
                  if (Array.isArray(body[field]))
                    body[field] = body[field].filter((row) => row.catalog_key !== 'a-docs');
                await route.fulfill({ response, body: JSON.stringify(body) });
              };
              const withoutCatalogSelected = async (route) => {
                const query = new URL(route.request().url()).searchParams;
                // This mock only changes the catalog listing, never a selected-session lookup.
                if (query.has('sid')) return route.continue();
                const response = await route.fetch();
                const body = await response.json();
                if (Array.isArray(body.items))
                  body.items = body.items.filter((row) => row.key !== 'a-docs');
                await route.fulfill({ response, body: JSON.stringify(body) });
              };
              const unavailableTranscript = async (route) => {
                const query = new URL(route.request().url()).searchParams;
                const response = await route.fetch();
                const body = await response.json();
                if (query.get('sid') === 'a-docs' && body.session?.key === 'a-docs') {
                  body.session.status = { state: null, observation: 'unavailable' };
                  body.session.details = {
                    state: 'done',
                    turns: 0,
                    calls: null,
                    errors: null,
                    tool_calls: {},
                    signals: {},
                    cost: { usd: null, unpriced_models: [] },
                    cwd: null,
                    copied_usage: false,
                    reported_runs: [],
                    cost_check: [],
                    ...body.session.details,
                    state: 'done',
                  };
                  sawUnavailableStatus = true;
                }
                await route.fulfill({ response, body: JSON.stringify(body) });
              };
              let sawUnavailableStatus = false;
              await page.route('**/api/viewer/home?**', withoutHomeSelected);
              await page.route('**/api/sessions?**', withoutCatalogSelected);
              await page.route('**/api/session-transcript?**', unavailableTranscript);
              await page.goto(address[1] + '/s/claude/a-docs?t=' + address[2] + '&compat=1');
              await page.locator('#page [aria-label="Transcript"]').waitFor();
              assert.equal(
                sawUnavailableStatus,
                true,
                'selected metadata reports unavailable status',
              );
              const selectedRecent = page.locator('#lanes .treeitem[data-id*="a-docs"]');
              await selectedRecent.waitFor({ state: 'visible' });
              const ingestRecent = page.locator('#lanes .treeitem[data-id*="atlas-ingest"]'),
                listedChildren = ingestRecent.locator(':scope > .tree-group > .treeitem');
              await listedChildren.nth(2).waitFor({ state: 'visible' });
              const listedChildIds = await listedChildren.evaluateAll((nodes) =>
                nodes.map((node) => node.getAttribute('data-id') ?? ''),
              );
              assert.equal(
                listedChildIds.length,
                3,
                'the selected child occupies one of the three Recent preview slots',
              );
              assert.ok(
                listedChildIds.some((id) => id.includes('a-docs')),
                'the selected child remains visible within the capped preview',
              );
              assert.match(
                await selectedRecent.innerText(),
                /Update the ingest docs/,
                'Recent keeps the selected child visible on its parent path',
              );
              assert.equal(
                await selectedRecent
                  .locator('.dot')
                  .evaluate((node) => node.classList.contains('off')),
                true,
                'unavailable selected status does not fall back to stale completed details',
              );
              const ingestSelector = '#lanes .treeitem[data-id*="atlas-ingest"]',
                ingestToggle = page.locator(ingestSelector + ' > .tree-row > .tree-toggle');
              await ingestToggle.focus();
              await ingestToggle.press('Enter');
              await page.locator(ingestSelector + '[aria-expanded="false"]').waitFor();
              assert.equal(
                await selectedRecent.isVisible(),
                false,
                'a keyboard collapse stays closed for the current navigation',
              );
              // Drain active async routes before navigation so no handler races its response.
              await page.unrouteAll({ behavior: 'wait' });
              await page.goto(address[1] + '/s/claude/a-review?t=' + address[2] + '&compat=1');
              await page.locator('#page [aria-label="Transcript"]').waitFor();
              await page.locator(ingestSelector + '[aria-expanded="true"]').waitFor();
              assert.equal(
                await page.locator(ingestSelector).getAttribute('aria-expanded'),
                'true',
                'the next navigation opens its required Recent ancestor despite saved collapse',
              );

              // A fresh document preserves the existing ranked order; verify selected-path
              // promotion separately while the same Recent ordering scope is still alive.
              await page.route('**/api/viewer/home?**', withoutHomeSelected);
              await page.route('**/api/sessions?**', withoutCatalogSelected);
              sawUnavailableStatus = false;
              await page.route('**/api/session-transcript?**', unavailableTranscript);
              await page.goto(address[1] + '/?t=' + address[2] + '&compat=1');
              await page.locator('#page h1').filter({ hasText: 'Home' }).waitFor();
              await ingestRecent.waitFor({ state: 'visible' });
              if ((await ingestRecent.getAttribute('aria-expanded')) !== 'true') {
                await ingestToggle.focus();
                await ingestToggle.press('Enter');
                await page.locator(ingestSelector + '[aria-expanded="true"]').waitFor();
              }
              const warmListedChildren = ingestRecent.locator(
                ':scope > .tree-group > .treeitem',
              );
              await warmListedChildren.nth(2).waitFor({ state: 'visible' });
              const warmInitialIds = await warmListedChildren.evaluateAll((nodes) =>
                nodes.map((node) => node.getAttribute('data-id') ?? ''),
              );
              assert.equal(warmInitialIds.length, 3, 'Home starts with three Recent preview rows');
              assert.equal(
                warmInitialIds.some((id) => id.includes('a-docs')),
                false,
                'the selected session starts outside the filtered Home preview',
              );

              await page.evaluate((path) => {
                history.pushState(history.state, '', path);
                window.dispatchEvent(new PopStateEvent('popstate'));
              }, '/s/claude/a-docs?t=' + address[2] + '&compat=1');
              await page.locator('#page [aria-label="Transcript"]').waitFor();
              assert.equal(
                sawUnavailableStatus,
                true,
                'the SPA-selected metadata reports unavailable status',
              );
              await selectedRecent.waitFor({ state: 'visible' });
              await warmListedChildren.nth(2).waitFor({ state: 'visible' });
              const promotedIds = await warmListedChildren.evaluateAll((nodes) =>
                nodes.map((node) => node.getAttribute('data-id') ?? ''),
              );
              assert.equal(promotedIds.length, 3, 'selected promotion keeps the three-row cap');
              assert.ok(
                promotedIds.some((id) => id.includes('a-docs')),
                'the selected child remains within the capped preview',
              );
              assert.ok(
                promotedIds[0]?.includes('a-docs'),
                'a newly selected child leads a retained Recent ordering scope',
              );
              await page.unrouteAll({ behavior: 'wait' });
              await page.goto(address[1] + '/?t=' + address[2] + '&compat=1');
              await page.locator('#page h1').filter({ hasText: 'Home' }).waitFor();
            }
            if (width === 1280) {
              await page.getByRole('button', { name: 'Collapse sidebar', exact: true }).click();
              await page.locator('.app.rail').waitFor();
              assert.ok(
                (await page.locator('#lanes .child-dot').count()) > 0,
                'collapsed rail retains recorded descendant activity',
              );
              assert.equal(await page.locator('#lanes .kid-flag').count(), 0);
              await page.getByRole('button', { name: 'Expand sidebar', exact: true }).click();
              await page.locator('.app:not(.rail)').waitFor();
            }

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
            for (const row of home.working.filter((row) => row.session.activity)) {
              const activity = row.session.activity;
              assert.ok(
                (
                  await page
                    .locator('#page .nrow')
                    .filter({ has: page.getByText(row.session.name, { exact: true }) })
                    .textContent()
                ).includes(activity[0]),
                'Home retains the recorded tool name in working activity',
              );
            }
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
            for (const machine of home.machines) {
              const machineRow = page
                .locator('#page .nrow[data-m]')
                .filter({ hasText: machine.label });
              await machineRow.waitFor();
              assert.equal(
                await machineRow.locator('.ag').textContent(),
                !machine.up ? 'offline' : machine.working ? 'up' : 'idle',
              );
              if (machine.up)
                assert.match(
                  await machineRow.locator('.for').textContent(),
                  new RegExp(machine.working + ' working.*' + machine.sessions + ' sessions?'),
                );
            }
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
            // Geometry comes from the selected complete application's styles
            // at 263a02d, including the phone overrides, not the deferred v6.
            const machineGeometry = await page.locator('#page').evaluate((node) => {
              const pageStyle = getComputedStyle(node),
                row = getComputedStyle(node.querySelector('.nrow'));
              return {
                padding: pageStyle.padding,
                gap: pageStyle.gap,
                rowPadding: row.padding,
                rowGap: row.columnGap,
                title: node.querySelector('.ph.sr h1')?.textContent,
              };
            });
            assert.deepEqual(machineGeometry, {
              padding: width === 390 ? '2px 16px 72px' : '8px 32px 88px',
              gap: width === 390 ? '16px' : '24px',
              rowPadding: '11px 0px',
              rowGap: '10px',
              title: home.machines[0].label,
            });
            const quill = page.locator('#page .nrow').filter({
              has: page.locator('.nm', { hasText: /^quill$/ }),
            });
            assert.match(await quill.locator('.for').textContent(), /for you.*Add PDF export/);
            assert.equal(
              await quill.locator('.ag .hicon').count(),
              0,
              'non-working rows retain their age without a harness icon',
            );
            assert.match(
              await page
                .locator('#page .nrow')
                .filter({ has: page.locator('.nm', { hasText: /^Principal$/ }) })
                .textContent(),
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
            const runView = page.getByRole('button', {
              name: 'Run view for feat/offline-sync',
              exact: true,
            });
            await page
              .getByRole('button', { name: 'Expand feat/offline-sync details', exact: true })
              .click();
            await runView.waitFor();
            const parentSession = new URL(page.url()).pathname.split('/').at(-1);
            await runView.click();
            await page.locator('#page h1').filter({ hasText: 'Trace' }).waitFor();
            assert.match(new URL(page.url()).pathname, /^\/trace\/native\//);
            assert.equal(
              new URL(page.url()).pathname.split('/')[3],
              parentSession,
              'Run view retains the sending session as Trace root',
            );
            await page.getByRole('region', { name: 'Agents', exact: true }).waitFor();
            await page.goBack();
            await page.locator('[aria-label="Transcript"]').waitFor();
            await page.locator('#page .child-card').first().waitFor();

            assert.equal(
              await page
                .getByText('Original action prompt is unavailable.', { exact: true })
                .count(),
              0,
            );
            assert.ok(await page.locator('#page [data-turn]').count());
            assert.match(
              await page.locator('#page .session-foot').textContent(),
              /Working.*5 tool calls.*1d 4h.*\$14\.04/,
            );
            await page
              .getByRole('button', { name: 'Open Check retry safety', exact: true })
              .click();
            await page.getByRole('button', { name: 'Open in harbor', exact: true }).waitFor();
            assert.match(
              await page.locator('#page .session-foot').textContent(),
              /Returned to harbor.*Failed/,
            );
            await page.getByRole('button', { name: 'Open in harbor', exact: true }).click();
            await page.locator('#page .child-card').first().waitFor();

            assert.equal(
              await page.locator('#page [aria-label="Conversation controls"]').count(),
              0,
            );
            assert.equal(
              await page.getByRole('button', { name: 'Open Composer', exact: true }).count(),
              0,
              'paused Composer is not advertised',
            );
            assert.equal(await page.locator('.session-composer-trigger').count(), 0);
            assert.equal(
              await page.getByRole('dialog', { name: 'Session Composer', exact: true }).count(),
              0,
            );
            assert.equal(await page.locator('#nav [data-go="new-session"]').count(), 0);
            const trigger = page.getByRole('button', {
              name: 'Session menu: details, cost and actions',
              exact: true,
            });
            await trigger.waitFor();
            assert.equal(await trigger.getAttribute('aria-expanded'), 'false');
            await trigger.click();
            const panel = page.getByRole('dialog', { name: /^Session menu for .+/ });
            await panel.waitFor();
            assert.equal(await trigger.getAttribute('aria-expanded'), 'true');
            await screenshot('session-menu');
            await page.keyboard.press('Escape');
            assert.equal(await panel.count(), 0);
            assert.equal(await trigger.getAttribute('aria-expanded'), 'false');
            assert.equal(await trigger.evaluate((node) => node === document.activeElement), true);
            await page.waitForFunction(() => !history.state?.sheet);
            const sessionUrl = page.url();
            await trigger.click();
            assert.equal(await page.evaluate(() => history.state?.sheet), 1);
            await page.goBack();
            await panel.waitFor({ state: 'detached' });
            assert.equal(page.url(), sessionUrl, 'Back closes the menu on the same session');
            assert.equal(await trigger.getAttribute('aria-expanded'), 'false');
            assert.equal(await trigger.evaluate((node) => node === document.activeElement), true);
            assert.equal(
              requests.includes('/api/control'),
              false,
              'reader issues no control writes',
            );
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
          } catch (error) {
            if (captureServedFixtureFailure)
              await captureServedFixtureFailure('context journey failure', {
                error:
                  error instanceof Error
                    ? { name: error.name.slice(0, 100), message: error.message.slice(0, 1000) }
                    : String(error).slice(0, 1000),
              }).catch(() => {
                console.error('[served fixture diagnostic unavailable]');
              });
            throw error;
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

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const { outputFiles } = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents:
      "import {installSelect} from './src/lib/select'; installSelect(); export {mountViewerApplication} from './src/app/viewer';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'CatalogViewer',
  platform: 'browser',
});
const html = (
  await readFile(new URL('../../crates/semon-sessions/src/viewer.html', import.meta.url), 'utf8')
)
  .replace('<script src="/viewer.js" defer></script>', '')
  .replace('<link rel="stylesheet" href="/shell.css">', '');
const css = (
  await Promise.all(
    ['viewer', 'shell', 'select'].map((name) =>
      readFile(new URL(`../../crates/semon-sessions/src/${name}.css`, import.meta.url), 'utf8'),
    ),
  )
).join('\n');
const source = (key) => ({
  root: 'claude',
  path: key + '.jsonl',
  native_id: 'native-' + key,
  offset: 0,
  prefix_sha256: Array(32).fill(1),
  tail_sha256: Array(32).fill(2),
});
const meta = (key) => ({
  lifecycle: 'current',
  key,
  name: 'Session ' + key,
  harness: 'claude',
  kind: 'Claude Code',
  native_ids: ['native-' + key],
  parent: null,
  parent_source: null,
  repo: '/repo',
  branch: null,
  model: 'native-model',
  effort: null,
  start: null,
  last: null,
  tokens: [0, 0, 0],
  cost: { usd: null, unpriced_models: [] },
  source_refs: [{ source: source(key), state: 'cached' }],
  freshness: { state: 'cached' },
});
const identity = (key) => ({
  source_key: 'source',
  catalog_key: key,
  harness: 'claude',
  native_id: 'native-' + key,
  native_ids: ['native-' + key],
  source_refs: meta(key).source_refs,
  machine_label: null,
  generation: 'a'.repeat(64),
  observed_at: null,
  freshness: { state: 'cached' },
});
const capabilities = (selected) => ({
  api: 1,
  read_contract: 'catalog-v1',
  source_key: 'source',
  selected_transcript: selected,
  selected_identity: true,
  selected_entry: false,
  attachment: false,
  relationship_context: false,
  large_native_records: false,
  pagination: true,
  filters: ['harness', 'repo'],
  order: 'last_desc_key_asc',
  full_text_search: false,
  global_union: false,
});
const list = (items) => ({
  api: 1,
  machine: 'source',
  read_scope: 'retained_history',
  machine_info: { key: 'source', label: 'Fixture source', freshness: 'cached' },
  generation: 'a'.repeat(64),
  observed_at: null,
  freshness: 'cached',
  capabilities: {
    pagination: true,
    filters: ['harness', 'repo'],
    order: 'last_desc_key_asc',
    full_text_search: false,
    selected_session_lookup: true,
    runtime_status: false,
    global_union: false,
  },
  items,
  next_cursor: null,
});
function transcript(key, p, total = 65, generation = 'b') {
  const limit = Number(p.get('limit') ?? 60),
    first = p.has('after') ? Number(p.get('after')) : Math.max(0, total - limit),
    end = Math.min(first + limit, total);
  return {
    api: 1,
    identity: { ...identity(key), read_scope: 'retained_history' },
    session: meta(key),
    projection: {
      version: 5,
      generation: generation.repeat(64),
      errors_generation: generation.repeat(64),
      total,
    },
    range: { first, end, next: end < total ? end : null },
    entries: Array.from({ length: end - first }, (_, i) => ({
      k: 'a',
      text: 'Original ' + key + ' record ' + (first + i),
      ...(first + i === 6
        ? { clipped: true, field: { name: 'text', chunks: 2, complete: false } }
        : {}),
      slot: first + i,
      entry_id: key + ':' + (first + i),
      provenance: { source: source(key), native_event_id: null, offset: first + i, block: 0 },
    })),
    freshness: { state: 'cached' },
    relationship_context: { state: 'incomplete', turn_ids: [], handoffs: [] },
  };
}
for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`catalog boot, selection and retained native ranges use no global model at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme }),
          requests = [];
        let nativeGeneration = 'b',
          nativeTotal = 65;
        await page.route('http://catalog.test/**', async (route) => {
          const u = new URL(route.request().url());
          requests.push(u.pathname + u.search);
          const font = u.pathname.match(
            /^\/fonts\/(instrument-sans|jetbrains-mono)-(latin(?:-ext)?)\.woff2$/,
          );
          if (font)
            return route.fulfill({
              contentType: 'font/woff2',
              body: await readFile(
                new URL(
                  '../../crates/semon-sessions/src/fonts/' + font[1] + '/' + font[2] + '.woff2',
                  import.meta.url,
                ),
              ),
            });
          if (u.pathname === '/viewer.css')
            return route.fulfill({ contentType: 'text/css', body: css });
          if (u.pathname === '/api/session-sources')
            return route.fulfill({
              json: {
                api: 1,
                items: [
                  { source_key: 'source', label: 'Same machine' },
                  { source_key: 'second', label: 'Same machine' },
                ],
                next_cursor: null,
              },
            });
          if (u.pathname === '/api/session-source-candidates')
            return route.fulfill({
              json: {
                api: 1,
                source_key: u.searchParams.get('machine') ?? 'source',
                items: [],
                next_cursor: null,
              },
            });
          if (u.pathname === '/api/session-capabilities')
            return route.fulfill({
              json: {
                ...capabilities(true),
                source_key: u.searchParams.get('machine') ?? 'source',
                selected_entry: true,
                source_candidates: true,
              },
            });
          if (u.pathname === '/api/session-entry') {
            const chunk = Number(u.searchParams.get('field_chunk')),
              key = u.searchParams.get('sid');
            return route.fulfill({
              json: {
                api: 1,
                identity: { ...identity(key), read_scope: 'retained_history' },
                projection: { version: 5, generation: nativeGeneration.repeat(64) },
                slot: 6,
                field: { name: 'text', chunk, next: chunk === 0 ? 1 : null, complete: chunk === 1 },
                text: chunk === 0 ? 'First native text chunk ' : 'and final native text chunk.',
                freshness: { state: 'cached' },
                provenance: { source: source(key), offset: 6, block: 0, native_event_id: null },
                observation: { source_bytes: 28 },
              },
            });
          }
          if (u.pathname === '/api/session-identity')
            return route.fulfill({
              json: {
                api: 1,
                identity: {
                  ...identity(u.searchParams.get('sid')),
                  source_key: u.searchParams.get('machine') ?? 'source',
                },
              },
            });
          if (u.pathname === '/api/sessions')
            return route.fulfill({
              json: {
                ...list(
                  [meta('one'), meta('two')].map((item) =>
                    u.searchParams.get('sid') === item.key
                      ? {
                          ...item,
                          transcript: {
                            generation: nativeGeneration.repeat(64),
                            errors_generation: nativeGeneration.repeat(64),
                            total: nativeTotal,
                          },
                        }
                      : item,
                  ),
                ),
                machine: u.searchParams.get('machine'),
                machine_info: {
                  key: u.searchParams.get('machine'),
                  label: 'Same machine',
                  freshness: 'cached',
                },
              },
            });
          if (u.pathname === '/api/session-transcript') {
            if (
              u.searchParams.has('generation') &&
              u.searchParams.get('generation') !== nativeGeneration.repeat(64)
            )
              return route.fulfill({
                status: 409,
                json: { error: 'stale_projection', resynchronize: true },
              });
            const value = transcript(
              u.searchParams.get('sid'),
              u.searchParams,
              nativeTotal,
              nativeGeneration,
            );
            value.identity.source_key = u.searchParams.get('machine');
            return route.fulfill({ json: value });
          }
          if (u.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
          return route.fulfill({ contentType: 'text/html', body: html });
        });
        await page.goto('http://catalog.test/sessions');
        await page.clock.install({ time: new Date('2026-10-07T00:00:00Z') });
        await page.clock.pauseAt(new Date('2026-10-07T00:01:00Z'));
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.streams = [];
          window.EventSource = class extends EventTarget {
            constructor(path) {
              super();
              this.closed = false;
              this.path = path;
              streams.push(this);
              queueMicrotask(() => this.emit());
            }
            close() {
              this.closed = true;
            }
            emit() {
              const p = Object.fromEntries(new URL(this.path, location.href).searchParams);
              this.dispatchEvent(
                new MessageEvent('control', {
                  data: JSON.stringify({
                    ...p,
                    revision: 'fresh',
                    control: {
                      thread: p.native_id,
                      generation: 'native-generation',
                      activeTurn: null,
                      connected: true,
                      capabilities: {
                        input: true,
                        steer: false,
                        interrupt: false,
                        commandApproval: false,
                        fileApproval: false,
                        questions: false,
                      },
                      reason: null,
                      requests: [],
                      actions: {},
                    },
                  }),
                }),
              );
            }
          };
          window.app = CatalogViewer.mountViewerApplication({
            machinesPath: '/machines',
            catalogControlStream: '/catalog/status',
            navigation: {
              leading: [
                {
                  key: 'new-session',
                  label: 'New Session',
                  href: '/sessions/new',
                  icon: 'M4 12h16',
                },
              ],
            },
            loadMachines: async () => {
              throw Error('No native inventory in this fixture');
            },
          });
        });
        await page.locator('#page [data-id="one"]').waitFor();
        assert.equal(await page.locator('#nav [data-go="new-session"]').count(), 0);
        await page.evaluate(() => document.fonts.ready);
        if (process.env.SEMON_CATALOG_OUT) {
          await mkdir(process.env.SEMON_CATALOG_OUT, { recursive: true });
          await page.screenshot({
            path: process.env.SEMON_CATALOG_OUT + '/list-' + width + '-' + colorScheme + '.png',
          });
        }
        if (width === 1280) await page.getByRole('button', { name: 'Collapse sidebar' }).click();
        await page.locator('#page [data-id="one"]').click();
        await page.locator('#page [data-entry-key="one:5"]').waitFor();
        const original = await page.evaluateHandle(() =>
          document.querySelector('#page [data-entry-key="one:5"]'),
        );
        assert.equal(await page.getByRole('button', { name: 'Open Composer' }).count(), 0);
        assert.equal(await page.evaluate(() => streams.length), 0);
        await page.getByRole('button', { name: 'Load recorded text', exact: true }).click();
        await page.getByRole('button', { name: 'Load more text', exact: true }).waitFor();
        assert.match(
          await page.locator('#page [data-entry-key="one:6"]').innerText(),
          /First native text chunk/,
        );
        assert.doesNotMatch(
          await page.locator('#page [data-entry-key="one:6"]').innerText(),
          /Original one record 6/,
        );
        await page.getByRole('button', { name: 'Load more text', exact: true }).click();
        await page.getByText('Complete recorded text loaded.', { exact: true }).waitFor();
        assert.match(
          await page.locator('#page [data-entry-key="one:6"]').innerText(),
          /and final native text chunk/,
        );
        await page.keyboard.press('Escape');
        await page.getByRole('button', { name: 'Load earlier records' }).first().click();
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        await page.keyboard.press('Escape');
        await page.evaluate(() => document.querySelector('#nav [data-go="sessions"]')?.click());
        await page.locator('#page [data-id="two"]').click();
        await page.locator('#page [data-entry-key="two:5"]').waitFor();
        if (width === 1280) {
          assert.equal(
            await page.locator('.app').evaluate((node) => node.classList.contains('rail')),
            true,
          );
          await page.getByRole('button', { name: 'Expand sidebar' }).waitFor();
        }
        await page.evaluate(() => document.querySelector('#nav [data-go="sessions"]')?.click());
        await page.locator('#page [data-id="one"]').click();
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('#page [data-entry-key="one:5"]'),
            original,
          ),
          true,
        );
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        const openMachineChooser = async () => {
          await page.evaluate(() => document.querySelector('#nav [data-go="sessions"]')?.click());
          await page
            .locator('#topbar')
            .getByRole('link', { name: 'Choose a machine', exact: true })
            .click();
        };
        await openMachineChooser();
        await page.locator('[data-source-key="second"]').click();
        await page.locator('#page [data-id="one"]').click();
        await page.locator('#page [data-entry-key="one:5"]').waitFor();
        await page.keyboard.press('Escape');
        await page.waitForFunction(() => !history.state?.sheet);
        await openMachineChooser();
        await page.locator('[data-source-key="source"]').click();
        await page.locator('#page [data-id="one"]').click();
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('#page [data-entry-key="one:5"]'),
            original,
          ),
          true,
        );
        await page.goBack();
        await page.locator('#page [data-id="one"]').waitFor();
        await page.goBack();
        await page.locator('[data-source-key="second"]').waitFor();
        assert.equal(new URL(page.url()).searchParams.get('choose_source'), '1');
        await page.goBack();
        await page.locator('#page [data-id="one"]').waitFor();
        await page.goBack();
        await page.locator('#page [data-entry-key="one:5"]').waitFor();
        await page.goForward();
        await page.locator('#page [data-id="one"]').waitFor();
        await page.goForward();
        await page.locator('[data-source-key="source"]').waitFor();
        await page.goForward();
        await page.locator('#page [data-id="one"]').waitFor();
        await page.goForward();
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        // Revisit the other source after restoring the source navigation entry.
        await page.keyboard.press('Escape');
        await page.waitForFunction(() => !history.state?.sheet);
        await page.goBack();
        await page.locator('#page [data-id="one"]').waitFor();
        await page.goBack();
        await page.locator('[data-source-key="second"]').waitFor();
        await page.goBack();
        await page.locator('#page [data-id="one"]').waitFor();
        await page.goBack();
        await page.locator('#page [data-entry-key="one:5"]').waitFor();
        await page.keyboard.press('Escape');
        await page.waitForFunction(() => !history.state?.sheet);
        await openMachineChooser();
        await page.locator('[data-source-key="source"]').click();
        await page.locator('#page [data-id="one"]').click();
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        if (process.env.SEMON_CATALOG_OUT)
          await page.screenshot({
            path: process.env.SEMON_CATALOG_OUT + '/selected-' + width + '-' + colorScheme + '.png',
          });
        await page
          .locator('#page [data-entry-key="one:0"]')
          .evaluate((node) => node.scrollIntoView({ block: 'start' }));
        const focusTarget = page.getByRole('button', {
          name: 'Session menu: details, cost and actions',
        });
        const focusNode = await focusTarget.elementHandle();
        await focusTarget.evaluate((node) => node.focus({ preventScroll: true }));
        const anchorBefore = await page
          .locator('#page [data-entry-key="one:0"]')
          .evaluate((node) => node.getBoundingClientRect().top);
        nativeGeneration = 'c';
        nativeTotal = 66;
        await page.clock.runFor(3100);
        await page.locator('#page [data-entry-key="one:65"]').waitFor();
        assert.equal(
          await page.evaluate((node) => node === document.activeElement, focusNode),
          true,
        );
        const anchorAfter = await page
          .locator('#page [data-entry-key="one:0"]')
          .evaluate((node) => node.getBoundingClientRect().top);
        assert.ok(Math.abs(anchorBefore - anchorAfter) < 2);
        assert.match(
          await page.locator('#page [data-entry-key="one:6"]').innerText(),
          /and final native text chunk/,
        );
        await page.getByRole('button', { name: 'Reload recorded text', exact: true }).waitFor();
        assert.equal(requests.filter((p) => p.startsWith('/api/session-entry')).length, 2);
        await page.keyboard.press('Escape');
        await page.getByRole('button', { name: /Jump to bottom/ }).click();
        await page.waitForFunction(
          () => document.querySelector('#page > div')?.getAttribute('aria-busy') === 'false',
        );
        await focusTarget.focus();
        nativeGeneration = 'd';
        nativeTotal = 67;
        await page.clock.runFor(3100);
        await page.locator('#page [data-entry-key="one:66"]').waitFor({ state: 'attached' });
        assert.equal(
          await page.evaluate((node) => node === document.activeElement, focusNode),
          true,
        );
        const endGap = await page.evaluate(() => {
          const main = document.querySelector('#main');
          return innerWidth <= 760
            ? document.documentElement.scrollHeight - scrollY - innerHeight
            : main.scrollHeight - main.scrollTop - main.clientHeight;
        });
        assert.ok(endGap < 2, 'Deliberate live following must reach the new end; gap=' + endGap);
        assert.equal(
          requests.some((p) => /^\/api\/(model|tool|image|tx)/.test(p)),
          false,
        );
        assert.ok(
          requests
            .filter((p) => p.startsWith('/api/session-transcript'))
            .every((p) =>
              ['source', 'second'].includes(
                new URL('http://catalog.test' + p).searchParams.get('machine'),
              ),
            ),
        );
        await page.evaluate(() => app.destroy());
      } finally {
        await browser.close();
      }
    });
test('temporary boot failure and pending projection recover without starting global content', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage(),
      requests = [];
    let capabilityReads = 0,
      ready = false,
      rangeReads = 0;
    await page.route('http://catalog.test/**', (route) => {
      const url = new URL(route.request().url()),
        p = url.pathname;
      requests.push(p);
      if (p === '/api/session-capabilities') {
        if (++capabilityReads === 1) return route.fulfill({ status: 503, body: '' });
        return route.fulfill({ json: capabilities(ready) });
      }
      if (p === '/api/sessions') return route.fulfill({ json: list([meta('one')]) });
      if (p === '/api/session-identity') return route.fulfill({ status: 404, body: '' });
      if (p === '/api/session-transcript') {
        if (++rangeReads === 1) return route.fulfill({ status: 503, body: '' });
        return route.fulfill({ json: transcript('one', url.searchParams) });
      }
      return route.fulfill({
        contentType: p === '/viewer.css' ? 'text/css' : 'text/html',
        body: p === '/viewer.css' ? css : html,
      });
    });
    await page.goto('http://catalog.test/sessions');
    await page.clock.install({ time: new Date('2026-10-07T00:00:00Z') });
    await page.clock.pauseAt(new Date('2026-10-07T00:01:00Z'));
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => {
      window.runtimeStreams = [];
      window.EventSource = class extends EventTarget {
        constructor(path) {
          super();
          this.closed = false;
          runtimeStreams.push(this);
          const scope = Object.fromEntries(new URL(path, location.href).searchParams);
          queueMicrotask(() =>
            this.dispatchEvent(
              new MessageEvent('runtime', {
                data: JSON.stringify({
                  ...scope,
                  revision: 'd'.repeat(64),
                  runtime: {
                    state: 'ended',
                    phase: 'released',
                    freshness: 'current',
                    presence: 'absent',
                  },
                  reason: null,
                }),
              }),
            ),
          );
        }
        close() {
          this.closed = true;
        }
      };
      window.app = CatalogViewer.mountViewerApplication({
        machinesPath: '/machines',
        catalogRuntimeStream: '/catalog/runtime',
        loadMachines: async () => {
          throw Error('No global inventory');
        },
      });
    });
    await page
      .getByText('The source is updating or temporarily unavailable.', { exact: false })
      .waitFor();
    await page.clock.runFor(1100);
    await page.locator('#page [data-id="one"]').click();
    assert.equal(await page.getByRole('button', { name: 'Open Composer' }).count(), 0);
    await page
      .getByRole('region', { name: 'Environment status' })
      .getByText('This session has ended.', { exact: true })
      .waitFor();
    await page
      .getByText('Current native session selection is unavailable.', { exact: false })
      .waitFor();
    assert.equal(await page.locator('.composer-unavailable').count(), 0);
    assert.equal(await page.locator('textarea').count(), 0);
    assert.equal(
      requests.some((p) => p === '/api/model' || p === '/api/session-transcript'),
      false,
    );
    await page.keyboard.press('Escape');
    ready = true;
    await page.clock.runFor(8100);
    await page
      .getByText('The source is updating or temporarily unavailable.', { exact: false })
      .waitFor();
    await page.clock.runFor(1100);
    await page.locator('#page [data-entry-key="one:5"]').waitFor();
    assert.equal(await page.getByRole('button', { name: 'Open Composer' }).count(), 0);
    await page
      .getByRole('region', { name: 'Environment status' })
      .getByText('This session has ended.', { exact: true })
      .waitFor();
    assert.equal(requests.includes('/api/model'), false);
    await page.evaluate(() => app.destroy());
    const afterDestroy = requests.length;
    await page.clock.runFor(30000);
    assert.equal(requests.length, afterDestroy);
    assert.equal(await page.evaluate(() => runtimeStreams.every((stream) => stream.closed)), true);
  } finally {
    await browser.close();
  }
});
for (const width of [390, 1280])
  test(`bounded source discovery chooses exact keys and ignores late inventory at ${width}px`, async () => {
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({ viewport: { width, height: 900 } }),
        requests = [];
      let inventoryReads = 0,
        releaseInventory,
        startedInventory;
      const inventoryStarted = new Promise((resolve) => {
        startedInventory = resolve;
      });
      await page.route('http://catalog.test/**', async (route) => {
        const url = new URL(route.request().url());
        requests.push(url.pathname + url.search);
        if (url.pathname === '/api/session-capabilities')
          return url.searchParams.has('machine')
            ? route.fulfill({
                json: { ...capabilities(true), source_key: url.searchParams.get('machine') },
              })
            : route.fulfill({
                status: 400,
                json: {
                  api: 1,
                  error: {
                    code: 'machine_scope_required',
                    message: 'Choose a machine',
                    retryable: false,
                  },
                },
              });
        if (url.pathname === '/api/session-sources') {
          if (++inventoryReads === 1) return route.fulfill({ status: 503, body: '' });
          if (url.searchParams.has('cursor')) {
            startedInventory();
            await new Promise((resolve) => {
              releaseInventory = resolve;
            });
            try {
              await route.fulfill({
                json: {
                  api: 1,
                  items: [{ source_key: 'late', label: 'Late machine' }],
                  next_cursor: null,
                },
              });
            } catch {}
            return;
          }
          return route.fulfill({
            json: {
              api: 1,
              items: [
                { source_key: 'one', label: 'Same hostname' },
                { source_key: 'two', label: 'Same hostname' },
              ],
              next_cursor: 'opaque',
            },
          });
        }
        if (url.pathname === '/api/sessions')
          return route.fulfill({
            json: {
              ...list([meta('one')]),
              machine: 'two',
              machine_info: { key: 'two', label: 'Same hostname', freshness: 'cached' },
            },
          });
        return route.fulfill({
          contentType: url.pathname === '/viewer.css' ? 'text/css' : 'text/html',
          body: url.pathname === '/viewer.css' ? css : html,
        });
      });
      await page.goto('http://catalog.test/sessions' + (width === 1280 ? '?choose_source=1' : ''));
      await page.clock.install({ time: new Date('2026-10-07T00:00:00Z') });
      await page.clock.pauseAt(new Date('2026-10-07T00:01:00Z'));
      await page.addScriptTag({ content: outputFiles[0].text });
      await page.evaluate(() => {
        window.app = CatalogViewer.mountViewerApplication({
          machinesPath: '/machines',
          catalogSources: true,
          loadMachines: async () => {
            throw Error('No global native inventory');
          },
        });
      });
      await page
        .getByText('The source is updating or temporarily unavailable.', { exact: false })
        .waitFor();
      assert.equal(requests.includes('/api/session-capabilities'), false);
      await page.clock.runFor(1100);
      await page.locator('[data-source-key="two"]').waitFor();
      assert.equal(
        await page.locator('#nav [data-go="machines"]').getAttribute('href'),
        '/machines',
      );
      if (width === 1280) await page.getByRole('button', { name: 'Collapse sidebar' }).click();
      await page.getByRole('button', { name: 'Load more machines', exact: true }).click();
      await inventoryStarted;
      await page.locator('[data-source-key="two"]').click();
      await page.locator('#page [data-id="one"]').waitFor();
      assert.equal(
        await page.locator('#nav [data-go="machines"]').getAttribute('href'),
        '/machines',
      );
      assert.equal(new URL(page.url()).searchParams.get('machine'), 'two');
      assert.equal(new URL(page.url()).searchParams.has('choose_source'), false);
      if (width === 1280) {
        await page.getByRole('button', { name: 'Expand sidebar' }).waitFor();
        assert.equal(
          await page.locator('.app').evaluate((node) => node.classList.contains('rail')),
          true,
        );
      }
      releaseInventory();
      await page.clock.runFor(100);
      assert.equal(await page.locator('[data-source-key="late"]').count(), 0);
      assert.equal(await page.locator('#page [data-id="one"]').count(), 1);
      assert.equal(
        requests.some((path) => /^\/api\/(model|tool|image|tx)/.test(path)),
        false,
      );
      assert.ok(
        requests
          .filter((path) => path.startsWith('/api/sessions?'))
          .every(
            (path) => new URL('http://catalog.test' + path).searchParams.get('machine') === 'two',
          ),
      );
      await page.evaluate(() => app.destroy());
    } finally {
      await browser.close();
    }
  });

test('partial catalog discovery completes while retaining uncommitted search focus', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    let reads = 0,
      complete = false;
    await page.route('http://catalog.test/**', (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path === '/api/session-capabilities')
        return route.fulfill({
          json: {
            ...capabilities(false),
            metadata_search: true,
            filters: ['harness', 'repo', 'q'],
          },
        });
      if (path === '/api/sessions') {
        reads++;
        return route.fulfill({
          json: {
            api: 1,
            machine: 'source',
            read_scope: 'retained_history',
            machine_info: { key: 'source', label: 'Source', freshness: 'cached' },
            generation: 'a'.repeat(64),
            observed_at: null,
            freshness: complete ? 'cached' : 'updating',
            completeness: { state: complete ? 'complete' : 'partial' },
            capabilities: {
              pagination: true,
              filters: ['harness', 'repo'],
              order: 'last_desc_key_asc',
              full_text_search: false,
              selected_session_lookup: true,
              runtime_status: false,
              global_union: false,
            },
            items: [meta('one')],
            next_cursor: null,
          },
        });
      }
      return route.fulfill({
        contentType: path === '/viewer.css' ? 'text/css' : 'text/html',
        body: path === '/viewer.css' ? css : html,
      });
    });
    await page.goto('http://catalog.test/sessions');
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => (window.app = CatalogViewer.mountViewerApplication()));
    await page.getByText('More sessions are being discovered. This list is incomplete.').waitFor();
    const harness = page.getByRole('searchbox', { name: 'Search session details', exact: true });
    await harness.fill('uncommitted');
    await harness.evaluate((node) => node.setSelectionRange(2, 7));
    complete = true;
    await page
      .getByText('More sessions are being discovered. This list is incomplete.')
      .waitFor({ state: 'hidden' });
    assert.equal(await harness.inputValue(), 'uncommitted');
    assert.equal(await harness.evaluate((node) => document.activeElement === node), true);
    assert.deepEqual(
      await harness.evaluate((node) => [node.selectionStart, node.selectionEnd]),
      [2, 7],
    );
    assert.ok(reads >= 2);
    await harness.press('Tab');
    assert.equal(await harness.inputValue(), 'uncommitted');
    await page.evaluate(() => app.destroy());
  } finally {
    await browser.close();
  }
});

test('archive source hint waits for a parsed canonical session without native authority or global restore', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage(),
      requests = [];
    let polls = 0;
    await page.route('http://catalog.test/**', (route) => {
      const u = new URL(route.request().url());
      requests.push(u.pathname + u.search);
      if (u.pathname === '/api/session-capabilities')
        return route.fulfill({ json: { ...capabilities(true), source_candidates: true } });
      if (u.pathname === '/api/sessions')
        return route.fulfill({ json: list(u.searchParams.has('sid') ? [meta('parsed')] : []) });
      if (u.pathname === '/api/session-source-candidates')
        return route.fulfill({
          json: {
            api: 1,
            source_key: 'source',
            items: [
              {
                candidate_key: 'candidate',
                source: { root: 'claude', path: 'archived.jsonl' },
                native_name_hint: 'Old native name hint',
                generation: 'c'.repeat(64),
                archive_observed_at: null,
              },
            ],
            next_cursor: null,
          },
        });
      if (u.pathname === '/api/session-source-candidate') {
        polls++;
        return route.fulfill({
          json: {
            api: 1,
            source_key: 'source',
            candidate_key: 'candidate',
            generation: 'c'.repeat(64),
            state: polls === 1 ? 'updating' : 'ready',
            catalog_key: polls === 1 ? null : 'parsed',
            retryable: true,
            reason: null,
          },
        });
      }
      if (u.pathname === '/api/session-transcript')
        return route.fulfill({ json: transcript('parsed', u.searchParams, 3) });
      if (u.pathname === '/api/session-identity')
        return route.fulfill({ status: 404, json: { error: 'Current authority unavailable' } });
      return route.fulfill({
        contentType: u.pathname === '/viewer.css' ? 'text/css' : 'text/html',
        body: u.pathname === '/viewer.css' ? css : html,
      });
    });
    await page.goto('http://catalog.test/sessions');
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => (window.app = CatalogViewer.mountViewerApplication()));
    const hint = page.getByRole('button', { name: /Old native name hint/ });
    await hint.waitFor();
    assert.equal(await page.locator('[data-id="candidate"]').count(), 0);
    assert.equal(
      requests.some((path) => path.startsWith('/api/session-identity')),
      false,
    );
    await hint.click();
    await page
      .getByText('Recorded source is being indexed. Its session will open automatically.')
      .waitFor();
    await page.getByText('Original parsed record 0', { exact: true }).waitFor();
    assert.equal(new URL(page.url()).pathname, '/s/claude/parsed');
    assert.ok(polls >= 2);
    assert.equal(
      requests.some((path) => /^\/api\/(model|tool|tx|image)(\?|$)/.test(path)),
      false,
    );
    await page.evaluate(() => app.destroy());
  } finally {
    await browser.close();
  }
});

test('bounded metadata search preserves literal query and exposes empty partial continuation', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage(),
      requests = [];
    let searches = 0;
    const query = 'Écho + & branch';
    await page.route('http://catalog.test/**', (route) => {
      const u = new URL(route.request().url());
      requests.push(u.pathname + u.search);
      if (u.pathname === '/api/session-capabilities')
        return route.fulfill({
          json: {
            ...capabilities(false),
            metadata_search: true,
            filters: ['harness', 'repo', 'q'],
          },
        });
      if (u.pathname === '/api/sessions') {
        if (!u.searchParams.has('q')) return route.fulfill({ json: list([meta('initial')]) });
        assert.equal(u.searchParams.get('q'), query);
        searches++;
        return route.fulfill({
          json: {
            ...list(searches === 1 ? [] : [meta('matched')]),
            next_cursor: searches === 1 ? 'continue' : null,
            search: {
              semantics: 'unicode_lowercase_substring',
              fields: ['name', 'key', 'repo', 'branch', 'model', 'harness'],
              partial: searches === 1,
              candidates: searches === 1 ? 512 : 1,
              index_complete: true,
              candidate_budget: 512,
              byte_budget: 2097152,
            },
          },
        });
      }
      return route.fulfill({
        contentType: u.pathname === '/viewer.css' ? 'text/css' : 'text/html',
        body: u.pathname === '/viewer.css' ? css : html,
      });
    });
    await page.goto('http://catalog.test/sessions');
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => (window.app = CatalogViewer.mountViewerApplication()));
    const input = page.getByRole('searchbox', { name: 'Search session details', exact: true });
    await input.waitFor();
    await input.fill(query);
    await input.press('Tab');
    await page
      .getByText(
        'Search checked a bounded part of the index. Load more sessions to continue looking for matches.',
      )
      .waitFor();
    assert.equal(await page.getByText('No recorded sessions match these filters.').count(), 0);
    await page.getByRole('button', { name: 'Load more sessions', exact: true }).click();
    await page.locator('#page [data-id="matched"]').waitFor();
    assert.equal(await input.inputValue(), query);
    assert.equal(searches, 2);
    assert.equal(
      requests.some((path) => /^\/api\/model/.test(path)),
      false,
    );
    await page.evaluate(() => app.destroy());
  } finally {
    await browser.close();
  }
});

for (const width of [390, 1280])
  test(`search submission and Back retain list context at ${width}px`, async () => {
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({ viewport: { width, height: 860 } });
      const reads = [];
      let fail = false;
      const items = Array.from({ length: 36 }, (_, i) => meta('row-' + i));
      await page.route('http://catalog.test/**', (route) => {
        const u = new URL(route.request().url());
        if (u.pathname === '/api/session-capabilities')
          return route.fulfill({
            json: {
              ...capabilities(true),
              metadata_search: true,
              filters: ['harness', 'repo', 'q'],
            },
          });
        if (u.pathname === '/api/sessions') {
          reads.push(Object.fromEntries(u.searchParams));
          if (fail) return route.fulfill({ status: 503, json: { error: 'unavailable' } });
          return route.fulfill({
            json: {
              ...list(u.searchParams.get('q') === 'missing' ? [] : items),
              ...(u.searchParams.has('q')
                ? {
                    search: {
                      semantics: 'unicode_lowercase_substring',
                      fields: ['name', 'key', 'repo', 'branch', 'model', 'harness'],
                      partial: false,
                      candidates: 36,
                      index_complete: true,
                      candidate_budget: 512,
                      byte_budget: 2097152,
                    },
                  }
                : {}),
            },
          });
        }
        if (u.pathname === '/api/session-identity')
          return route.fulfill({ json: { api: 1, identity: identity(u.searchParams.get('sid')) } });
        if (u.pathname === '/api/session-transcript')
          return route.fulfill({ json: transcript(u.searchParams.get('sid'), u.searchParams, 10) });
        return route.fulfill({
          contentType: u.pathname === '/viewer.css' ? 'text/css' : 'text/html',
          body: u.pathname === '/viewer.css' ? css : html,
        });
      });
      await page.goto('http://catalog.test/sessions');
      await page.addScriptTag({ content: outputFiles[0].text });
      await page.evaluate(() => (window.app = CatalogViewer.mountViewerApplication()));
      const query = page.getByRole('searchbox', { name: 'Search session details', exact: true });
      await query.waitFor();
      // Filter choices commit together when the existing shared sheet closes.
      await page.locator('.facet-btn').click();
      for (const [label, value] of [
        ['Harness', 'claude'],
        ['Repo', '/repo'],
      ]) {
        const select = page.locator(`.sh-select[data-label="${label}"]`);
        await select.locator('.sh-select-trigger').click();
        await select.locator(`[role="option"][data-value="${value}"]`).click();
      }
      const committed = page.waitForResponse((response) => {
        const url = new URL(response.url());
        return (
          url.pathname === '/api/sessions' &&
          url.searchParams.get('harness') === 'claude' &&
          url.searchParams.get('repo') === '/repo'
        );
      });
      await page.locator('.filters-sheet .fdone').click();
      await committed;
      await page.waitForFunction(() => !history.state?.sheet);
      assert.equal(reads.length, 2);
      await query.fill('Session');
      const searched = page.waitForResponse((response) => {
        const url = new URL(response.url());
        return url.pathname === '/api/sessions' && url.searchParams.get('q') === 'Session';
      });
      await query.press('Enter');
      await searched;
      await page.getByRole('button', { name: 'Clear filters', exact: true }).waitFor();
      await page.waitForFunction(() => new URLSearchParams(location.search).get('q') === 'Session');
      assert.equal(reads.length, 3);
      assert.equal(reads[2].q, 'Session');
      assert.equal(reads[2].harness, 'claude');
      assert.equal(reads[2].repo, '/repo');
      const row = page.locator('#page [data-id="row-18"]');
      await row.scrollIntoViewIfNeeded();
      const listScroll = await page.evaluate(() =>
        matchMedia('(max-width: 760px)').matches
          ? scrollY
          : document.querySelector('#main').scrollTop,
      );
      assert.ok(listScroll > 100);
      await row.press('Enter');
      await page.locator('[data-entry-key]').first().waitFor();
      if (width < 761) await page.locator('#lead-btn').click();
      await page.getByRole('link', { name: 'Sessions', exact: true }).press('Enter');
      await row.waitFor();
      assert.equal(await query.inputValue(), 'Session');
      assert.equal(await row.evaluate((node) => document.activeElement === node), true);
      const returnedScroll = await page.evaluate(() =>
        matchMedia('(max-width: 760px)').matches
          ? scrollY
          : document.querySelector('#main').scrollTop,
      );
      assert.ok(Math.abs(returnedScroll - listScroll) <= 2, `${returnedScroll} vs ${listScroll}`);
      await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
      await page
        .getByRole('button', { name: 'Clear filters', exact: true })
        .waitFor({ state: 'hidden' });
      assert.equal(await query.inputValue(), '');
      assert.equal(await query.evaluate((node) => document.activeElement === node), true);
      assert.equal(reads.at(-1).q, undefined);
      assert.equal(reads.at(-1).repo, undefined);
      await query.fill('missing');
      await query.press('Enter');
      await page.getByText('No recorded sessions match these filters.').waitFor();
      assert.equal(
        await page.getByRole('button', { name: 'Clear filters', exact: true }).count(),
        1,
      );
      fail = true;
      await query.fill('Session');
      await query.press('Enter');
      await page.getByRole('button', { name: 'Retry', exact: true }).waitFor();
      fail = false;
      await page.getByRole('button', { name: 'Retry', exact: true }).click();
      await row.waitFor();
      assert.equal(
        await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
        false,
      );
      await page.evaluate(() => app.destroy());
    } finally {
      await browser.close();
    }
  });
for (const width of [390, 1280])
  test(`late name metadata updates list Recent and header without transcript reads at ${width}px`, async () => {
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({ viewport: { width, height: 860 } });
      let name = 'Initial native name',
        reads = 0;
      await page.route('http://catalog.test/**', (route) => {
        const url = new URL(route.request().url());
        if (url.pathname === '/api/session-capabilities')
          return route.fulfill({ json: capabilities(true) });
        if (url.pathname === '/api/sessions')
          return route.fulfill({ json: list([{ ...meta('one'), name }]) });
        if (url.pathname === '/api/session-identity')
          return route.fulfill({ json: { api: 1, identity: identity('one') } });
        if (url.pathname === '/api/session-transcript') {
          reads++;
          return route.fulfill({
            json: {
              ...transcript('one', url.searchParams),
              session: { ...meta('one'), name: 'Initial native name' },
            },
          });
        }
        return route.fulfill({
          contentType: url.pathname === '/viewer.css' ? 'text/css' : 'text/html',
          body: url.pathname === '/viewer.css' ? css : html,
        });
      });
      await page.goto('http://catalog.test/sessions');
      await page.clock.install({ time: new Date('2026-10-10T00:00:00Z') });
      await page.clock.pauseAt(new Date('2026-10-10T00:01:00Z'));
      await page.addScriptTag({ content: outputFiles[0].text });
      await page.evaluate(() => {
        window.app = CatalogViewer.mountViewerApplication();
      });
      await page.locator('[data-session-row="list"][data-id="one"]').click();
      await page.locator('[data-entry-key]').first().waitFor();
      // Suppress only the existing content poll: name observations must work independently.
      await page.evaluate(() =>
        Object.defineProperty(document, 'visibilityState', {
          configurable: true,
          get: () => 'hidden',
        }),
      );
      const firstReads = reads;
      await page.evaluate(() => {
        window.body = document.querySelector('.turns');
        window.position = document.querySelector('#main').scrollTop;
      });
      name = 'Meaningful later name';
      await page.clock.runFor(8100);
      await page.waitForFunction(
        () => document.querySelector('#topbar .l1 .t')?.textContent === 'Meaningful later name',
      );
      assert.equal(reads, firstReads);
      assert.equal(await page.evaluate(() => body === document.querySelector('.turns')), true);
      assert.match(await page.locator('#side-list').textContent(), /Meaningful later name/);
      // Unchanged content can still carry its old brief during a later content poll.
      await page.evaluate(() =>
        Object.defineProperty(document, 'visibilityState', {
          configurable: true,
          get: () => 'visible',
        }),
      );
      await page.clock.runFor(3100);
      assert.equal(await page.locator('#topbar .l1 .t').textContent(), 'Meaningful later name');
      assert.equal(await page.evaluate(() => body === document.querySelector('.turns')), true);
      if (width < 761) await page.locator('#lead-btn').click();
      await page.getByRole('link', { name: 'Sessions', exact: true }).click();
      assert.match(
        await page.locator('[data-session-row="list"][data-id="one"] .nm').innerText(),
        /Meaningful later name/,
      );
    } finally {
      await browser.close();
    }
  });

for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`complete Catalog rows and grouping stay focused at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme });
        const requests = [],
          children = Array.from({ length: 7 }, (_, i) => ({
            ...meta('child-' + i),
            parent: 'parent',
            presentation_kind: 'Subagent',
          })),
          items = [meta('parent'), ...children, { ...meta('orphan'), parent: 'unloaded-parent' }];
        await page.route('http://catalog.test/**', (route) => {
          const u = new URL(route.request().url());
          if (u.pathname.startsWith('/api/')) requests.push(u.pathname + u.search);
          if (u.pathname === '/api/session-capabilities')
            return route.fulfill({ json: capabilities(false) });
          if (u.pathname === '/api/sessions') return route.fulfill({ json: list(items) });
          if (u.pathname === '/api/session-identity')
            return route.fulfill({
              json: { ...identity(u.searchParams.get('sid')), read_scope: 'retained_history' },
            });
          return route.fulfill({
            contentType: u.pathname === '/viewer.css' ? 'text/css' : 'text/html',
            body: u.pathname === '/viewer.css' ? css : html,
          });
        });
        await page.goto('http://catalog.test/sessions');
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.app = CatalogViewer.mountViewerApplication();
          Object.defineProperty(document, 'visibilityState', {
            configurable: true,
            get: () => 'hidden',
          });
        });
        const parent = page.locator('[data-session-row="list"][data-id="parent"]');
        await parent.waitFor();
        assert.equal(await page.locator('[data-session-row="list"]').count(), 9);
        assert.equal(await page.locator('[data-id="orphan"][data-session-row="list"]').count(), 1);
        await parent.evaluate((node) => {
          window.parentRow = node;
        });
        const group = page.getByRole('button', { name: 'Project', exact: true });
        const before = requests.length;
        await group.focus();
        await group.press('Enter');
        assert.equal(await group.getAttribute('aria-pressed'), 'true');
        assert.equal(await group.evaluate((node) => node === document.activeElement), true);
        assert.equal(await page.locator('[data-session-row="list"]').count(), 9);
        await page.getByRole('button', { name: 'Recent', exact: true }).press('Enter');
        await parent.evaluate((node) => {
          window.parentRow = node;
        });
        await page.getByRole('button', { name: 'Recent', exact: true }).press('Enter');
        assert.equal(
          await page.evaluate(
            () =>
              parentRow === document.querySelector('[data-session-row="list"][data-id="parent"]'),
          ),
          true,
        );
        assert.equal(requests.length, before, 'local grouping requested source preparation');
        const child = page.locator('[data-session-row="list"][data-id="child-0"]');
        assert.equal(await child.locator('.session-row-meta').count(), 1);
        assert.equal(await child.locator('.row-delegation').count(), 1);
        assert.ok(
          await child.evaluate(
            (node, minimum) => node.getBoundingClientRect().height >= minimum,
            width < 761 ? 44 : 36,
          ),
        );
        await child.press('Enter');
        await page.waitForFunction(
          () => history.state?.v === 'session' && history.state.id === 'child-0',
        );
        assert.equal(
          requests.some(
            (path) => path.startsWith('/api/model') || path.startsWith('/api/session-transcript'),
          ),
          false,
        );
        await page.evaluate(() => app.destroy());
      } finally {
        await browser.close();
      }
    });

async function fulfillCatalogStatic(route, url) {
  const font = url.pathname.match(
    /^\/fonts\/(instrument-sans|jetbrains-mono)-(latin(?:-ext)?)\.woff2$/,
  );
  if (font)
    return route.fulfill({
      contentType: 'font/woff2',
      body: await readFile(
        new URL(
          '../../crates/semon-sessions/src/fonts/' + font[1] + '/' + font[2] + '.woff2',
          import.meta.url,
        ),
      ),
    });
  return route.fulfill({
    contentType: url.pathname === '/viewer.css' ? 'text/css' : 'text/html',
    body: url.pathname === '/viewer.css' ? css : html,
  });
}

function withDeadline(promise, label, timeoutMs = 10000) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error('Timed out waiting for ' + label)), timeoutMs);
    }),
  ]).finally(() => clearTimeout(timer));
}

for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`late bounded context preserves reading position before scroll delivery at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      let releasePreview;
      const previewGate = new Promise((resolve) => {
        releasePreview = resolve;
      });
      try {
        const page = await browser.newPage({
          viewport: { width, height: 900 },
          colorScheme,
          isMobile: width === 390,
          hasTouch: width === 390,
        });
        let signalPreview;
        const previewStarted = new Promise((resolve) => {
          signalPreview = resolve;
        });
        const previewGeneration = 'c'.repeat(64);
        await page.route('http://catalog.test/**', async (route) => {
          const url = new URL(route.request().url());
          if (url.pathname === '/api/session-capabilities')
            return route.fulfill({ json: capabilities(true) });
          if (url.pathname === '/api/sessions') return route.fulfill({ json: list([meta('one')]) });
          if (url.pathname === '/api/session-transcript') {
            const value = transcript('one', url.searchParams, 60);
            Object.assign(value.entries.at(-1), { k: 'h', id: 'child-edge', turn: 'parent-turn' });
            delete value.entries.at(-1).text;
            value.relationship_context = {
              state: 'complete',
              turn_ids: ['parent-turn'],
              turns: [
                {
                  sid: 'one',
                  id: 'parent-turn',
                  u: false,
                  end: { st: 'done', why: 'complete' },
                  sent: ['child-edge'],
                  last: true,
                },
              ],
              handoffs: [
                {
                  id: 'child-edge',
                  kind: 'spawn',
                  ask: 'task',
                  from: 'one',
                  to: 'child',
                  at: 1,
                  done: 2,
                  status: 'done',
                  declined: false,
                  brief_session: 'child',
                  source_turn: 'parent-turn',
                },
              ],
              sessions: {
                child: {
                  name: 'Child',
                  harness: 'claude',
                  model: 'native-model',
                  kind: 'subagent',
                  state: 'done',
                },
              },
              previews: { child: previewGeneration },
            };
            return route.fulfill({ json: value });
          }
          if (url.pathname === '/api/session-home') {
            signalPreview();
            await previewGate;
            return route.fulfill({
              json: {
                api: 1,
                identity: { ...identity('child'), read_scope: 'retained_history' },
                generation: previewGeneration,
                incomplete: false,
                events: { 'child-edge': { brief: 'Hydrated bounded child brief' } },
                session: {},
              },
            });
          }
          if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
          return fulfillCatalogStatic(route, url);
        });
        await page.goto('http://catalog.test/sessions');
        // Defer only delivery to the application's scroll listener. Native
        // geometry and input remain real, modeling a response before that
        // asynchronous event reaches the owner without a timing-based retry.
        await page.evaluate(() => {
          window.deferReaderScroll = false;
          window.deferredReaderScrolls = 0;
          window.readerWheelObserved = false;
          window.addEventListener(
            'scroll',
            (event) => {
              if (
                window.deferReaderScroll &&
                (event.target === document || event.target === document.querySelector('#main'))
              ) {
                window.deferredReaderScrolls++;
                event.stopImmediatePropagation();
              }
            },
            { capture: true, passive: true },
          );
          document.addEventListener('wheel', (event) => {
            if (event.isTrusted) window.readerWheelObserved = true;
          });
        });
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.app = CatalogViewer.mountViewerApplication();
        });
        await page.locator('#page [data-id="one"]').click();
        await withDeadline(previewStarted, 'bounded child context request');
        await page.locator('#page [data-entry-key="one:59"]').waitFor();
        await page.evaluate(() => document.fonts.ready);
        const inert = await page.locator('#topbar').evaluate((bar) => {
          const rect = bar.getBoundingClientRect(),
            x = Math.min(rect.right - 1, innerWidth - 1),
            y = Math.max(rect.top + 1, 1),
            target = document.elementFromPoint(x, y);
          if (!bar.contains(target) || target.closest('button, a, input, [role="button"]'))
            throw Error('Reader input requires inert topbar padding');
          return { x, y };
        });
        await page.mouse.move(inert.x, inert.y);
        await page.mouse.wheel(0, 1);
        await page.waitForFunction(() => window.readerWheelObserved);
        const before = await page.locator('#page [data-entry-key="one:20"]').evaluate((node) => {
          window.deferReaderScroll = true;
          node.scrollIntoView({ block: 'center' });
          const main = document.querySelector('#main');
          return {
            top: node.getBoundingClientRect().top,
            gap:
              innerWidth <= 760
                ? document.documentElement.scrollHeight - scrollY - innerHeight
                : main.scrollHeight - main.scrollTop - main.clientHeight,
          };
        });
        assert.ok(before.gap > 80, 'The native reader must move away from the tail');
        await page.waitForFunction(() => window.deferredReaderScrolls > 0);
        releasePreview();
        await page
          .getByText('Hydrated bounded child brief', { exact: true })
          .waitFor({ state: 'attached' });
        const after = await page.locator('#page [data-entry-key="one:20"]').evaluate((node) => ({
          top: node.getBoundingClientRect().top,
          deferred: window.deferredReaderScrolls,
        }));
        assert.ok(after.deferred > 0, 'The reading-scroll event must remain deferred');
        assert.ok(Math.abs(after.top - before.top) < 2, JSON.stringify({ before, after }));
        await page.evaluate(() => {
          window.deferReaderScroll = false;
          window.dispatchEvent(new Event('scroll'));
          document.querySelector('#main').dispatchEvent(new Event('scroll'));
        });
        await page.getByRole('button', { name: /Jump to bottom/ }).waitFor();
        await page.evaluate(() => app.destroy());
      } finally {
        releasePreview();
        await browser.close();
      }
    });

test('cached history restores tail following independently of another reader position', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage({
        viewport: { width: 1280, height: 900 },
        colorScheme: 'dark',
      }),
      observations = { one: { generation: 'b', total: 65 }, two: { generation: 'b', total: 65 } };
    let returningToB = false;
    await page.route('http://catalog.test/**', async (route) => {
      const url = new URL(route.request().url()),
        key = url.searchParams.get('sid');
      if (url.pathname === '/api/session-capabilities')
        return route.fulfill({ json: capabilities(true) });
      if (url.pathname === '/api/session-identity')
        return route.fulfill({ json: { api: 1, identity: identity(key) } });
      if (url.pathname === '/api/sessions') {
        const keys = key ? [key] : ['one', 'two'];
        return route.fulfill({
          json: list(
            keys.map((key) => ({
              ...meta(key),
              ...(key === 'two' && returningToB ? { name: 'Restored session two' } : {}),
              transcript: {
                generation: observations[key].generation.repeat(64),
                errors_generation: observations[key].generation.repeat(64),
                total: observations[key].total,
              },
            })),
          ),
        });
      }
      if (url.pathname === '/api/session-transcript') {
        const { generation, total } = observations[key];
        if (
          url.searchParams.has('generation') &&
          url.searchParams.get('generation') !== generation.repeat(64)
        )
          return route.fulfill({
            status: 409,
            json: { error: 'stale_projection', resynchronize: true },
          });
        return route.fulfill({ json: transcript(key, url.searchParams, total, generation) });
      }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
      return fulfillCatalogStatic(route, url);
    });
    await page.goto('http://catalog.test/sessions');
    await page.clock.install({ time: new Date('2026-10-17T00:00:00Z') });
    await page.clock.pauseAt(new Date('2026-10-17T00:01:00Z'));
    await page.evaluate(() => {
      window.deferHistoryScroll = false;
      window.addEventListener(
        'scroll',
        (event) => {
          if (window.deferHistoryScroll && event.target === document.querySelector('#main'))
            event.stopImmediatePropagation();
        },
        { capture: true, passive: true },
      );
    });
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => {
      window.app = CatalogViewer.mountViewerApplication();
    });
    const gap = () =>
      page
        .locator('#main')
        .evaluate((node) => node.scrollHeight - node.scrollTop - node.clientHeight);
    await page.locator('#page [data-id="two"]').click();
    await page.locator('#page [data-entry-key="two:64"]').waitFor();
    await page.evaluate(() => document.fonts.ready);
    await page.clock.runFor(34);
    assert.ok((await gap()) < 2, 'Reader B initially follows the tail');
    await page.locator('#nav [data-go="sessions"]').click();
    await page.locator('#page [data-id="one"]').click();
    await page.locator('#page [data-entry-key="one:64"]').waitFor();
    const inert = await page.locator('#topbar').evaluate((bar) => {
      const rect = bar.getBoundingClientRect(),
        x = rect.right - 1,
        y = rect.top + 1,
        target = document.elementFromPoint(x, y);
      if (!bar.contains(target) || target.closest('button, a, input, [role="button"]'))
        throw Error('History input requires inert topbar padding');
      return { x, y };
    });
    await page.mouse.click(inert.x, inert.y);
    await page
      .locator('#page [data-entry-key="one:20"]')
      .evaluate((node) => node.scrollIntoView({ block: 'center' }));
    await page.clock.runFor(34);
    await page.getByRole('button', { name: /Jump to bottom/ }).waitFor();
    await page
      .getByRole('button', { name: 'Session menu: details, cost and actions' })
      .evaluate((node) => node.focus({ preventScroll: true }));
    const anchor = await page
      .locator('#page [data-entry-key="one:20"]')
      .evaluate((node) => node.getBoundingClientRect().top);
    // Keep the native restoration real while preventing a later scroll event
    // from concealing an incorrectly demoted cached following flag.
    await page.evaluate(() => {
      window.deferHistoryScroll = true;
    });
    await page.goBack();
    await page.locator('#page [data-id="one"]').waitFor();
    returningToB = true;
    await page.goBack();
    await page.locator('#page [data-entry-key="two:64"]').waitFor();
    // Retained readers refresh metadata on the existing selected observer.
    // Advance its controlled clock; restoration itself reuses cached metadata.
    await page.clock.runFor(3100);
    // The accepted metadata title proves the restoration's asynchronous
    // observer finished before the next projection is appended.
    await page
      .locator('#page .ph > h1')
      .filter({ hasText: 'Restored session two' })
      .waitFor({ state: 'attached' });
    assert.ok((await gap()) < 2, 'History restores B to its saved tail');
    assert.equal(
      await page.locator('#page .ph > h1').evaluate((node) => node === document.activeElement),
      true,
    );
    observations.two = { generation: 'c', total: 75 };
    await page.clock.runFor(3100);
    await page.locator('#page [data-entry-key="two:74"]').waitFor({ state: 'attached' });
    assert.ok((await gap()) < 2, 'Restored B follows the appended bounded projection');
    await page.evaluate(() => {
      window.deferHistoryScroll = false;
      document.querySelector('#main').dispatchEvent(new Event('scroll'));
    });
    await page.goForward();
    await page.locator('#page [data-id="one"]').waitFor();
    await page.goForward();
    await page.locator('#page [data-entry-key="one:20"]').waitFor();
    assert.equal(
      await page.locator('#page .ph > h1').evaluate((node) => node === document.activeElement),
      true,
    );
    const restored = await page
      .locator('#page [data-entry-key="one:20"]')
      .evaluate((node) => node.getBoundingClientRect().top);
    assert.ok(Math.abs(restored - anchor) < 2, JSON.stringify({ anchor, restored }));
    await page.evaluate(() => app.destroy());
  } finally {
    await browser.close();
  }
});

test('same-projection turn anchors retain loaded fields, focus and the anchor inside the bounded cache', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } }),
      requests = [],
      transcriptReads = [];
    let anchorReads = 0,
      anchorRangeReads = 0,
      releaseFirstAnchor,
      signalFirstAnchor,
      releaseStaleAnchor,
      signalStaleAnchor;
    const firstAnchorStarted = new Promise((resolve) => {
      signalFirstAnchor = resolve;
    });
    const staleAnchorStarted = new Promise((resolve) => {
      signalStaleAnchor = resolve;
    });
    await page.route('http://catalog.test/**', async (route) => {
      const url = new URL(route.request().url());
      requests.push(url.pathname + url.search);
      if (url.pathname === '/api/session-capabilities')
        return route.fulfill({ json: { ...capabilities(true), selected_entry: true } });
      if (url.pathname === '/api/sessions') return route.fulfill({ json: list([meta('one')]) });
      if (url.pathname === '/api/session-identity') return route.fulfill({ status: 404, body: '' });
      if (url.pathname === '/api/session-anchor') {
        const call = ++anchorReads;
        if (call === 1) {
          signalFirstAnchor();
          await new Promise((resolve) => {
            releaseFirstAnchor = resolve;
          });
        }
        if (call === 3) {
          signalStaleAnchor();
          await new Promise((resolve) => {
            releaseStaleAnchor = resolve;
          });
        }
        const newer = call === 4;
        return route.fulfill({
          json: {
            api: 1,
            source_key: 'source',
            catalog_key: 'one',
            turn: url.searchParams.get('turn'),
            slot: newer ? 20 : 0,
            generation: (newer ? 'd' : 'b').repeat(64),
          },
        });
      }
      if (url.pathname === '/api/session-transcript') {
        const query = url.searchParams,
          after = query.get('after'),
          generation = query.get('generation') === 'd'.repeat(64) ? 'd' : 'b';
        transcriptReads.push({
          after,
          limit: query.get('limit'),
          generation,
          errorsGeneration: query.get('errors_generation'),
        });
        if (anchorReads > 0 && after === '0' && ++anchorRangeReads === 1)
          return route.fulfill({ status: 409, json: { error: 'stale_projection' } });
        const errorsGeneration =
          query.get('errors_generation') === 'c'.repeat(64) || (anchorReads === 2 && after === '0')
            ? 'c'
            : generation;
        const value = transcript('one', query, 600, generation);
        value.projection.errors_generation = errorsGeneration.repeat(64);
        for (const row of value.entries) {
          if (row.slot === 0) {
            row.turn = 'anchor-turn';
            row.text = 'Anchor target record';
          }
          if (generation === 'd' && row.slot === 20) {
            row.turn = 'anchor-turn';
            row.text = 'New generation anchor slot';
          }
          if (row.slot === 400) {
            row.clipped = true;
            row.field = { name: 'text', chunks: 2, complete: false };
          }
        }
        if (value.entries.some((row) => row.turn === 'anchor-turn'))
          value.relationship_context = {
            state: 'complete',
            turn_ids: ['anchor-turn'],
            turns: [
              {
                sid: 'one',
                id: 'anchor-turn',
                u: false,
                end: { st: 'done', why: 'complete' },
                sent: [],
                last: true,
              },
            ],
            handoffs: [],
            sessions: {},
            previews: {},
          };
        value.identity.source_key = query.get('machine') ?? 'source';
        return route.fulfill({ json: value });
      }
      if (url.pathname === '/api/session-entry') {
        const chunk = Number(url.searchParams.get('field_chunk')),
          text = chunk === 0 ? 'First expansion chunk ' : 'Final field segment.';
        return route.fulfill({
          json: {
            api: 1,
            identity: { ...identity('one'), read_scope: 'retained_history' },
            projection: { version: 5, generation: 'b'.repeat(64) },
            slot: 400,
            field: { name: 'text', chunk, next: chunk === 0 ? 1 : null, complete: chunk === 1 },
            text,
            observation: { source_bytes: Buffer.byteLength(text) },
            freshness: { state: 'cached' },
            provenance: { source: source('one'), offset: 400, block: 0, native_event_id: null },
          },
        });
      }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
      return fulfillCatalogStatic(route, url);
    });
    await page.goto('http://catalog.test/sessions');
    await page.clock.install({ time: new Date('2026-10-17T00:00:00Z') });
    await page.clock.pauseAt(new Date('2026-10-17T00:01:00Z'));
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => {
      window.app = CatalogViewer.mountViewerApplication();
    });
    await page.locator('#page [data-id="one"]').click();
    await page.locator('#page [data-entry-key="one:599"]').waitFor();
    for (const first of [440, 340]) {
      await page.getByRole('button', { name: 'Load earlier records' }).click();
      await page.locator(`#page [data-entry-key="one:${first}"]`).waitFor();
    }
    const field = page.locator('#page [data-entry-key="one:400"]');
    await page.getByRole('button', { name: 'Load recorded text', exact: true }).click();
    await page.getByRole('button', { name: 'Load more text', exact: true }).click();
    await page.getByText('Complete recorded text loaded.', { exact: true }).waitFor();
    await page.getByRole('button', { name: 'Load earlier records' }).click();
    await page.locator('#page [data-entry-key="one:240"]').waitFor();
    assert.equal(await page.locator('#page [data-entry-key]').count(), 300);
    await page.locator('#main').evaluate((node) => {
      node.scrollTop = 500;
    });
    const scrollBefore = await page.locator('#main').evaluate((node) => node.scrollTop);
    await page.evaluate(() => {
      const state = { v: 'session', id: 'one', turn: 'anchor-turn' };
      history.pushState(state, '', '/s/claude/one?machine=source#turn=anchor-turn');
      dispatchEvent(new PopStateEvent('popstate', { state }));
    });
    await withDeadline(firstAnchorStarted, 'first turn anchor request');
    await page.clock.runFor(34);
    assert.equal(
      await page.evaluate(
        () => document.activeElement === document.querySelector('#page .ph > h1'),
      ),
      true,
    );
    assert.equal(await page.locator('#main').evaluate((node) => node.scrollTop), scrollBefore);
    const menu = page.getByRole('button', { name: 'Session menu: details, cost and actions' });
    await menu.focus();
    releaseFirstAnchor();
    await page.locator('#page [data-entry-key="one:0"]').waitFor();
    assert.match(
      await page.locator('#page [data-entry-key="one:0"]').innerText(),
      /Anchor target record/,
    );
    assert.match(await field.innerText(), /Final field segment\./);
    assert.equal(anchorReads, 2, 'one 409 may re-resolve the same turn once');
    assert.equal(await menu.evaluate((node) => node === document.activeElement), true);
    assert.deepEqual(
      transcriptReads
        .filter((read) => read.errorsGeneration === 'c'.repeat(64))
        .map(({ after, limit }) => [after, limit]),
      [
        ['240', '100'],
        ['340', '100'],
        ['440', '100'],
      ],
    );
    assert.ok(
      transcriptReads
        .filter((read) => read.errorsGeneration === 'c'.repeat(64))
        .every((read) => read.generation === 'b'),
      'range recovery keeps the same native generation while accepting the new errors generation',
    );
    assert.equal(
      transcriptReads.filter((read) => read.after === '0').length,
      2,
      're-accepting the validated anchor must not issue another transcript read',
    );
    assert.equal(await page.locator('#page [data-entry-key]').count(), 300);

    // A stale response for this same turn must not survive a session-list round trip.
    await page.evaluate(() => {
      const state = { v: 'session', id: 'one', turn: 'anchor-turn' };
      history.pushState(state, '', '/s/claude/one?machine=source#turn=anchor-turn');
      dispatchEvent(new PopStateEvent('popstate', { state }));
    });
    await withDeadline(staleAnchorStarted, 'superseded turn anchor request');
    await page.evaluate(() => document.querySelector('#nav [data-go="sessions"]')?.click());
    await page.locator('#page [data-id="one"]').waitFor();
    await page.evaluate(() => {
      const state = { v: 'session', id: 'one', turn: 'anchor-turn' };
      history.pushState(state, '', '/s/claude/one?machine=source#turn=anchor-turn');
      dispatchEvent(new PopStateEvent('popstate', { state }));
    });
    await page.getByText('New generation anchor slot', { exact: true }).waitFor();
    assert.equal(anchorReads, 4);
    const readsBeforeStaleResponse = transcriptReads.length;
    releaseStaleAnchor();
    await page.clock.runFor(34);
    assert.equal(anchorReads, 4, 'the older same-turn navigation did not resolve again');
    assert.equal(transcriptReads.length, readsBeforeStaleResponse);
    assert.equal(await page.getByText('New generation anchor slot', { exact: true }).count(), 1);
    await page.evaluate(() => window.app.destroy());
  } finally {
    await browser.close();
  }
});

test('a lost transcript capability cancels a pending turn anchor before any range request', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage(),
      requests = [];
    let capabilityReads = 0,
      releaseAnchor,
      signalAnchorStarted,
      signalMetadataAfterLoss;
    const anchorStarted = new Promise((resolve) => {
        signalAnchorStarted = resolve;
      }),
      metadataAfterLoss = new Promise((resolve) => {
        signalMetadataAfterLoss = resolve;
      });
    await page.route('http://catalog.test/**', async (route) => {
      const url = new URL(route.request().url());
      requests.push(url.pathname + url.search);
      if (url.pathname === '/api/session-capabilities') {
        const selected = ++capabilityReads === 1;
        return route.fulfill({ json: capabilities(selected) });
      }
      if (url.pathname === '/api/sessions') {
        if (capabilityReads > 1) signalMetadataAfterLoss();
        return route.fulfill({ json: list([meta('one')]) });
      }
      if (url.pathname === '/api/session-anchor') {
        signalAnchorStarted();
        await new Promise((resolve) => {
          releaseAnchor = resolve;
        });
        return route.fulfill({
          json: {
            api: 1,
            source_key: 'source',
            catalog_key: 'one',
            turn: url.searchParams.get('turn'),
            slot: 12,
            generation: 'b'.repeat(64),
          },
        });
      }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
      return fulfillCatalogStatic(route, url);
    });
    await page.goto('http://catalog.test/s/claude/one?machine=source#turn=pending-turn');
    await page.clock.install({ time: new Date('2026-10-17T00:00:00Z') });
    await page.clock.pauseAt(new Date('2026-10-17T00:01:00Z'));
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => {
      window.app = CatalogViewer.mountViewerApplication();
    });
    await withDeadline(anchorStarted, 'pending turn anchor request');
    await page.clock.runFor(8100);
    await withDeadline(metadataAfterLoss, 'metadata read after capability loss');
    releaseAnchor();
    await page.clock.runFor(34);
    assert.equal(capabilityReads, 2);
    assert.equal(
      requests.some((request) => request.startsWith('/api/session-transcript')),
      false,
      'a superseded anchor did not issue a transcript read after capability loss',
    );
    await page.evaluate(() => window.app.destroy());
  } finally {
    await browser.close();
  }
});

test('child activity reads stay separate from event previews and honor participant state and generation', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } }),
      edges = [
        {
          id: 'activity',
          sid: 'active-child',
          name: 'Active child',
          state: 'work',
          preview: 'brief',
        },
        {
          id: 'unavailable',
          sid: 'unavailable-child',
          name: 'Unavailable preview child',
          state: 'work',
          preview: 'brief',
        },
        {
          id: 'tool-mismatch',
          sid: 'tool-mismatch-child',
          name: 'Wrong tool child',
          state: 'work',
          preview: 'brief',
        },
        {
          id: 'since-mismatch',
          sid: 'since-mismatch-child',
          name: 'Wrong time child',
          state: 'work',
          preview: 'brief',
        },
        {
          id: 'result',
          sid: 'result-child',
          name: 'Result child',
          state: 'work',
          preview: 'result',
        },
        { id: 'done', sid: 'done-child', name: 'Done child', state: 'done' },
      ],
      participantActivity = {
        'active-child': { tool: 'Bash', since: 1000 },
        'unavailable-child': { tool: 'Read', since: 2000 },
        'tool-mismatch-child': { tool: 'Bash', since: 3000 },
        'since-mismatch-child': { tool: 'Bash', since: 4000 },
        'result-child': { tool: 'Bash', since: 5000 },
        'done-child': { tool: 'Edit', since: 6000 },
      },
      activityTuples = {
        'active-child': ['Bash', 'echo active work', null, 1000],
        'unavailable-child': ['Read', 'cat active work', null, 2000],
        'tool-mismatch-child': ['Read', 'wrong tool', null, 3000],
        'since-mismatch-child': ['Bash', 'wrong since', null, 4999],
        'result-child': ['Bash', 'result already recorded', null, 5000],
      },
      homeRequests = [],
      anchorTurns = [];
    let homeResponses = 0,
      signalHomeResponses,
      signalParentAnchor,
      signalActivityEvent,
      releaseActivityEvent;
    const allHomeResponses = new Promise((resolve) => {
      signalHomeResponses = resolve;
    });
    const parentAnchorStarted = new Promise((resolve) => {
      signalParentAnchor = resolve;
    });
    const activityEventStarted = new Promise((resolve) => {
      signalActivityEvent = resolve;
    });
    const previewGeneration = 'c'.repeat(64),
      childDetails = {
        state: 'done',
        turns: 1,
        calls: 0,
        errors: 0,
        tool_calls: {},
        signals: {},
        cost: { usd: null, unpriced_models: [] },
        copied_usage: false,
        copilot: null,
        reported_runs: [],
        cost_check: [],
      },
      relationships = edges.map(({ id, sid, preview }, index) => ({
        id,
        kind: 'spawn',
        from: 'parent',
        to: sid,
        at: index + 1,
        ...(id === 'done' ? { done: 20 } : {}),
        status: id === 'done' ? 'done' : 'work',
        declined: false,
        ...(preview === 'brief' ? { brief_session: sid } : {}),
        ...(preview === 'result' ? { result_session: sid } : {}),
        source_turn: 'parent-turn',
        target_turn: sid + '-turn',
      })),
      sessions = Object.fromEntries(
        edges.map(({ sid, name, state }) => [
          sid,
          {
            name,
            harness: 'claude',
            model: 'native-model',
            kind: 'subagent',
            state,
            live_observation: state === 'work',
            activity: participantActivity[sid],
          },
        ]),
      ),
      previews = Object.fromEntries(edges.map(({ sid }) => [sid, previewGeneration]));
    await page.route('http://catalog.test/**', async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname === '/api/session-capabilities')
        return route.fulfill({ json: capabilities(true) });
      if (url.pathname === '/api/sessions') return route.fulfill({ json: list([meta('parent')]) });
      if (url.pathname === '/api/session-identity') return route.fulfill({ status: 404, body: '' });
      if (url.pathname === '/api/session-anchor') {
        const turn = url.searchParams.get('turn');
        anchorTurns.push(turn);
        if (turn === 'parent-turn') signalParentAnchor();
        return route.fulfill({
          json: {
            api: 1,
            source_key: 'source',
            catalog_key: url.searchParams.get('sid'),
            turn,
            slot: 0,
            generation: 'b'.repeat(64),
          },
        });
      }
      if (url.pathname === '/api/session-transcript') {
        const sid = url.searchParams.get('sid');
        if (sid === 'done-child') {
          const value = transcript(sid, url.searchParams, 1, 'b');
          value.session = { ...meta(sid), details: childDetails };
          Object.assign(value.entries[0], { k: 'h', id: 'done', turn: 'done-child-turn' });
          delete value.entries[0].text;
          value.relationship_context = {
            state: 'complete',
            turn_ids: ['done-child-turn'],
            turns: [
              {
                sid,
                id: 'done-child-turn',
                start: 'done',
                u: false,
                end: { st: 'done', why: 'complete' },
                sent: [],
                last: true,
              },
            ],
            handoffs: [relationships.find((edge) => edge.id === 'done')],
            sessions: {
              parent: {
                name: 'Parent run',
                harness: 'claude',
                model: 'native-model',
                kind: 'codex-run',
                state: 'done',
              },
              'done-child': sessions['done-child'],
            },
            previews: {},
          };
          return route.fulfill({ json: value });
        }
        const value = transcript('parent', url.searchParams, edges.length, 'b');
        value.entries.forEach((row, index) => {
          Object.assign(row, { k: 'h', id: edges[index].id, turn: 'parent-turn' });
          delete row.text;
        });
        value.relationship_context = {
          state: 'complete',
          turn_ids: ['parent-turn'],
          turns: [
            {
              sid: 'parent',
              id: 'parent-turn',
              start: 'activity',
              u: false,
              end: { st: 'done', why: 'complete' },
              sent: edges.map((edge) => edge.id),
              last: true,
            },
          ],
          handoffs: relationships,
          sessions,
          previews,
        };
        return route.fulfill({ json: value });
      }
      if (url.pathname === '/api/session-home') {
        const sid = url.searchParams.get('sid'),
          eventIds = JSON.parse(url.searchParams.get('events') ?? '[]'),
          sessionFields = JSON.parse(url.searchParams.get('session_fields') ?? '[]'),
          unavailable = eventIds.includes('unavailable');
        homeRequests.push({ sid, eventIds, sessionFields, unavailable });
        const events = {};
        for (const id of eventIds) {
          if (id === 'activity') {
            signalActivityEvent();
            await new Promise((resolve) => {
              releaseActivityEvent = resolve;
            });
            events[id] = { brief: 'Native event brief' };
          } else if (id === 'tool-mismatch' || id === 'since-mismatch')
            events[id] = { brief: 'Mismatch preview' };
          else if (id === 'result') events[id] = { result: 'Recorded child result' };
        }
        const response = {
          api: 1,
          identity: { ...identity(sid), read_scope: 'retained_history' },
          generation: previewGeneration,
          incomplete: unavailable,
          events,
          session: sessionFields.includes('activity') ? { activity: activityTuples[sid] } : {},
        };
        await route.fulfill({ json: response });
        if (++homeResponses === 10) signalHomeResponses();
        return;
      }
      if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
      return fulfillCatalogStatic(route, url);
    });
    await page.goto('http://catalog.test/sessions');
    await page.clock.install({ time: new Date('2026-10-17T00:00:00Z') });
    await page.clock.pauseAt(new Date('2026-10-17T00:01:00Z'));
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => {
      window.app = CatalogViewer.mountViewerApplication();
    });
    await page.locator('#page [data-id="parent"]').click();
    await withDeadline(activityEventStarted, 'native edge event preview request');
    await page.locator('.child-card[data-h="activity"] .cc-now').waitFor({ state: 'attached' });
    releaseActivityEvent();
    await withDeadline(allHomeResponses, 'child event and activity home responses');
    const expand = async (id) => {
      const card = page.locator(`.child-card[data-h="${id}"]`);
      await card.locator('.cc-toggle').click();
      return card;
    };
    const active = await expand('activity');
    assert.match(await active.locator('.cc-brief').innerText(), /Native event brief/);
    assert.match(await active.locator('.cc-now').innerText(), /echo active work/);
    const unavailable = await expand('unavailable');
    assert.match(await unavailable.locator('.cc-now').innerText(), /cat active work/);
    const wrongTool = await expand('tool-mismatch');
    assert.equal(await wrongTool.locator('.cc-now').count(), 0);
    const wrongSince = await expand('since-mismatch');
    assert.equal(await wrongSince.locator('.cc-now').count(), 0);
    const result = await expand('result');
    assert.match(await result.locator('.cc-result').innerText(), /Recorded child result/);
    assert.equal(await result.locator('.cc-now').count(), 0);
    const done = await expand('done');
    assert.equal(await done.locator('.cc-now').count(), 0);
    const activityReads = homeRequests.filter(
      ({ eventIds, sessionFields }) => eventIds.length === 0 && sessionFields.includes('activity'),
    );
    assert.deepEqual(activityReads.map((request) => request.sid).sort(), [
      'active-child',
      'result-child',
      'since-mismatch-child',
      'tool-mismatch-child',
      'unavailable-child',
    ]);
    assert.equal(
      homeRequests.some(
        (request) => request.sid === 'done-child' && request.sessionFields.includes('activity'),
      ),
      false,
    );
    assert.equal(
      homeRequests.some(
        (request) =>
          request.sid === 'unavailable-child' &&
          request.eventIds.length === 1 &&
          request.unavailable,
      ),
      true,
      'an unavailable event preview is independent from the child activity read',
    );
    assert.equal(
      homeRequests.some(
        (request) => request.sid === 'active-child' && request.eventIds.includes('activity'),
      ),
      true,
      'the native edge named activity keeps its event read alongside the separate activity read',
    );
    await done.locator('.cc-name').click();
    await page
      .getByRole('button', { name: 'Open Parent run where it sent this', exact: true })
      .waitFor();
    await page.getByRole('button', { name: 'Open in Parent run', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Open in Parent run', exact: true }).click();
    await withDeadline(parentAnchorStarted, 'parent turn anchor request');
    await page.locator('#page [data-entry-key="parent:0"]').waitFor();
    assert.deepEqual(anchorTurns, ['done-child-turn', 'parent-turn']);
    assert.equal(new URL(page.url()).hash, '#turn=parent-turn');
    await page.evaluate(() => window.app.destroy());
  } finally {
    await browser.close();
  }
});

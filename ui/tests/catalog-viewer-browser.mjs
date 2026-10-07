import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const { outputFiles } = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents: "export {mountViewerApplication} from './src/app/viewer';",
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
).replace('<script src="/viewer.js" defer></script>', '');
const css = await readFile(
  new URL('../../crates/semon-sessions/src/viewer.css', import.meta.url),
  'utf8',
);
const source = (key) => ({
  root: 'claude',
  path: key + '.jsonl',
  native_id: 'native-' + key,
  offset: 0,
  prefix_sha256: Array(32).fill(1),
  tail_sha256: Array(32).fill(2),
});
const meta = (key) => ({
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
    projection: { version: 5, generation: generation.repeat(64), total },
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
          if (u.pathname === '/api/session-capabilities')
            return route.fulfill({
              json: {
                ...capabilities(true),
                source_key: u.searchParams.get('machine') ?? 'source',
                selected_entry: true,
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
                ...list([meta('one'), meta('two')]),
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
            loadMachines: async () => {
              throw Error('No native inventory in this fixture');
            },
          });
        });
        await page.locator('#page [data-id="one"]').waitFor();
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
        const draft = page.locator('#page textarea');
        await draft.fill('Retained native draft');
        await draft.focus();
        const draftNode = await draft.elementHandle();
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
        assert.equal(await draft.inputValue(), 'Retained native draft');
        await page.getByRole('button', { name: 'Load earlier records' }).first().click();
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        assert.equal(await draft.inputValue(), 'Retained native draft');
        await page.evaluate(() => document.querySelector('#nav a, #nav button')?.click());
        await page.locator('#page [data-id="two"]').click();
        await page.locator('#page [data-entry-key="two:5"]').waitFor();
        if (width === 1280) {
          assert.equal(
            await page.locator('.app').evaluate((node) => node.classList.contains('rail')),
            true,
          );
          await page.getByRole('button', { name: 'Expand sidebar' }).waitFor();
        }
        await page.evaluate(() => document.querySelector('#nav a, #nav button')?.click());
        await page.locator('#page [data-id="one"]').click();
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('#page [data-entry-key="one:5"]'),
            original,
          ),
          true,
        );
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        await page.evaluate(() => document.querySelector('#nav [data-go="sources"]').click());
        await page.locator('[data-source-key="second"]').click();
        await page.locator('#page [data-id="one"]').click();
        await page.locator('#page [data-entry-key="one:5"]').waitFor();
        await page.locator('#page textarea').fill('Second source draft');
        await page.evaluate(() => document.querySelector('#nav [data-go="sources"]').click());
        await page.locator('[data-source-key="source"]').click();
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        assert.equal(await page.locator('#page textarea').inputValue(), 'Retained native draft');
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('#page [data-entry-key="one:5"]'),
            original,
          ),
          true,
        );
        await page.goBack();
        await page.locator('#page [data-entry-key="one:5"]').waitFor();
        assert.equal(await page.locator('#page textarea').inputValue(), 'Second source draft');
        await page.goForward();
        await page.locator('#page [data-entry-key="one:0"]').waitFor();
        assert.equal(await page.locator('#page textarea').inputValue(), 'Retained native draft');
        if (process.env.SEMON_CATALOG_OUT)
          await page.screenshot({
            path: process.env.SEMON_CATALOG_OUT + '/selected-' + width + '-' + colorScheme + '.png',
          });
        assert.equal(await page.locator('#page textarea').inputValue(), 'Retained native draft');
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('#page textarea'),
            draftNode,
          ),
          true,
        );
        await page
          .locator('#page [data-entry-key="one:0"]')
          .evaluate((node) => node.scrollIntoView({ block: 'start' }));
        await page
          .locator('#page textarea')
          .evaluate((node) => node.focus({ preventScroll: true }));
        const anchorBefore = await page
          .locator('#page [data-entry-key="one:0"]')
          .evaluate((node) => node.getBoundingClientRect().top);
        nativeGeneration = 'c';
        nativeTotal = 66;
        await page.clock.runFor(3100);
        await page.locator('#page [data-entry-key="one:65"]').waitFor();
        assert.equal(await page.locator('#page textarea').inputValue(), 'Retained native draft');
        assert.equal(
          await page.evaluate((node) => node === document.activeElement, draftNode),
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
        await page.getByRole('button', { name: /Jump to bottom/ }).click();
        await page.waitForFunction(
          () => document.querySelector('#page > div')?.getAttribute('aria-busy') === 'false',
        );
        await page.locator('#page textarea').focus();
        nativeGeneration = 'd';
        nativeTotal = 67;
        await page.clock.runFor(3100);
        await page.locator('#page [data-entry-key="one:66"]').waitFor({ state: 'attached' });
        assert.equal(await page.locator('#page textarea').inputValue(), 'Retained native draft');
        assert.equal(
          await page.evaluate((node) => node === document.activeElement, draftNode),
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
    await page.getByText('Session history is unavailable.', { exact: false }).waitFor();
    await page.clock.runFor(1100);
    await page.locator('#page [data-id="one"]').click();
    await page.getByText('This session has ended.', { exact: true }).waitFor();
    await page
      .getByText('Current native session selection is unavailable.', { exact: false })
      .waitFor();
    assert.equal(await page.locator('textarea').count(), 0);
    assert.equal(
      requests.some((p) => p === '/api/model' || p === '/api/session-transcript'),
      false,
    );
    ready = true;
    await page.clock.runFor(8100);
    await page
      .getByText('The source is updating or temporarily unavailable.', { exact: false })
      .waitFor();
    await page.clock.runFor(1100);
    await page.locator('#page [data-entry-key="one:5"]').waitFor();
    await page.getByText('This session has ended.', { exact: true }).waitFor();
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
      await page.goto('http://catalog.test/sessions');
      await page.clock.install({ time: new Date('2026-10-07T00:00:00Z') });
      await page.clock.pauseAt(new Date('2026-10-07T00:01:00Z'));
      await page.addScriptTag({ content: outputFiles[0].text });
      await page.evaluate(() => {
        window.app = CatalogViewer.mountViewerApplication({
          machinesPath: '/machines',
          loadMachines: async () => {
            throw Error('No global native inventory');
          },
        });
      });
      await page.getByText('Source inventory is unavailable.', { exact: false }).waitFor();
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

test('partial catalog discovery refresh retains uncommitted filter focus and then completes', async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    let reads = 0;
    await page.route('http://catalog.test/**', (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path === '/api/session-capabilities') return route.fulfill({ json: capabilities(false) });
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
            freshness: reads === 1 ? 'updating' : 'cached',
            completeness: { state: reads === 1 ? 'partial' : 'complete' },
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
    const harness = page.getByRole('textbox', { name: 'Harness', exact: true });
    await harness.fill('uncommitted');
    await page.waitForTimeout(1500);
    assert.equal(reads, 1);
    assert.equal(await harness.inputValue(), 'uncommitted');
    assert.equal(await harness.evaluate((node) => document.activeElement === node), true);
    await harness.press('Tab');
    await page
      .getByText('More sessions are being discovered. This list is incomplete.')
      .waitFor({ state: 'hidden' });
    assert.equal(await harness.inputValue(), 'uncommitted');
    assert.ok(reads >= 2);
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
    const input = page.getByRole('textbox', { name: 'Search session details', exact: true });
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

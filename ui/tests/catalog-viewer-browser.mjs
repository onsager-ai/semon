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
    projection: { version: 1, generation: generation.repeat(64), total },
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
          if (u.pathname === '/api/session-capabilities')
            return route.fulfill({ json: { ...capabilities(true), selected_entry: true } });
          if (u.pathname === '/api/session-entry') {
            const chunk = Number(u.searchParams.get('field_chunk')),
              key = u.searchParams.get('sid');
            return route.fulfill({
              json: {
                api: 1,
                identity: { ...identity(key), read_scope: 'retained_history' },
                projection: { version: 1, generation: nativeGeneration.repeat(64) },
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
              json: { api: 1, identity: identity(u.searchParams.get('sid')) },
            });
          if (u.pathname === '/api/sessions')
            return route.fulfill({ json: list([meta('one'), meta('two')]) });
          if (u.pathname === '/api/session-transcript') {
            if (
              u.searchParams.has('generation') &&
              u.searchParams.get('generation') !== nativeGeneration.repeat(64)
            )
              return route.fulfill({
                status: 409,
                json: { error: 'stale_projection', resynchronize: true },
              });
            return route.fulfill({
              json: transcript(
                u.searchParams.get('sid'),
                u.searchParams,
                nativeTotal,
                nativeGeneration,
              ),
            });
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
            .every(
              (p) => new URL('http://catalog.test' + p).searchParams.get('machine') === 'source',
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

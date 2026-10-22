import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, mkdir } from 'node:fs/promises';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const { outputFiles } = await build({
  absWorkingDir: new URL('../', import.meta.url).pathname,
  stdin: {
    contents:
      "import {installSelect} from './src/lib/select'; import {installTooltip} from './src/lib/tooltip'; installSelect(); installTooltip(); export {mountViewerApplication} from './src/app/viewer';",
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

for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark'])
    for (const workspaceChrome of [false, true])
      test(`native host chrome preserves forms with workspace chrome=${workspaceChrome} at ${width}px ${colorScheme}`, async () => {
        const browser = await chromium.launch(),
          page = await browser.newPage({
            viewport: { width, height: 900 },
            colorScheme,
            isMobile: width === 390,
            hasTouch: width === 390,
          }),
          requests = [],
          errors = [];
        let publication = 0;
        try {
          page.setDefaultTimeout(10000);
          page.on('pageerror', (error) => errors.push(String(error)));
          await page.route('http://catalog.test/**', (route) => {
            const url = new URL(route.request().url());
            if (url.pathname.startsWith('/api/')) {
              requests.push(url.pathname + url.search);
              if (workspaceChrome && url.pathname === '/api/viewer/home') {
                publication++;
                return route.fulfill({
                  json: {
                    api: 1,
                    version: 'a'.repeat(64),
                    now: 10000,
                    next: { inbox: null, working: null, answered: null },
                    totals: { waiting: 2, working: 0, answered: 0 },
                    machines: [
                      {
                        key: 'source-0',
                        label: 'Alpha',
                        up: true,
                        working: 0,
                        sessions: 1,
                        last: 10000,
                      },
                    ],
                    recent: [
                      {
                        id: 'recent',
                        source_key: 'source-0',
                        catalog_key: 'recent',
                        session: {
                          name: 'Workspace Recent ' + publication,
                          harness: 'claude',
                          model: '',
                          repo: null,
                          state: 'idle',
                          last: 10000,
                          start: 9000,
                          content_generation: 'b'.repeat(64),
                        },
                      },
                    ],
                    inbox: [],
                    working: [],
                    answered: [],
                    observations: [],
                    pending_observations:
                      publication === 1 ? [] : [{ source_key: 'source-0', catalog_key: 'pending' }],
                  },
                });
              }
              return route.fulfill({
                status: 503,
                json: { error: 'unexpected_native_chrome_read' },
              });
            }
            return fulfillCatalogStatic(route, url);
          });
          await page.goto('http://catalog.test/workspace/settings?machine=source-0');
          await page.clock.install({ time: new Date('2026-10-18T00:00:00Z') });
          await page.clock.pauseAt(new Date('2026-10-18T00:00:00.001Z'));
          await page.addScriptTag({ content: outputFiles[0].text });
          await page.evaluate((workspaceChrome) => {
            const element = document.createElement('section');
            element.innerHTML =
              '<form action="/native-save" method="post"><input name="task" value="Retained task"><button>Save native task</button></form>';
            document.querySelector('#page').append(element);
            window.nativeContent = element;
            window.nativeInput = element.querySelector('input');
            window.nativeDestroys = 0;
            window.app = CatalogViewer.mountViewerApplication({
              machinesPath: '/manage/machines',
              nativePage: { title: 'Workspace settings', nav: 'machines', workspaceChrome },
              initialMachines: {
                element,
                destroy() {
                  window.nativeDestroys++;
                  element.remove();
                },
              },
            });
          }, workspaceChrome);
          const assertNative = async () => {
            if (workspaceChrome) {
              await page.locator('#lanes .srow').first().waitFor({ state: 'attached' });
              assert.match(await page.locator('#lanes').textContent(), /Workspace Recent/);
            }
            assert.equal(
              await page.evaluate(
                () => nativeContent === document.querySelector('#page > section'),
              ),
              true,
            );
            assert.equal(
              await page.evaluate(() => nativeInput === document.querySelector('#page input')),
              true,
            );
            assert.equal(await page.locator('#lanes .srow').count(), workspaceChrome ? 1 : 0);
            assert.equal(await page.locator('#page .catalog-session').count(), 0);
            assert.deepEqual(
              requests.filter((path) => !workspaceChrome || path !== '/api/viewer/home'),
              [],
              'native chrome may read compact workspace metadata, never select or prepare history',
            );
            assert.deepEqual(errors, []);
          };
          await assertNative();
          if (workspaceChrome)
            assert.equal(await page.locator('#nav [data-go="home"] .cnt').textContent(), '2');
          assert.equal(await page.locator('#nav [data-go="home"]').getAttribute('href'), '/');
          assert.equal(
            await page.locator('#nav [data-go="analytics"]').getAttribute('href'),
            '/analytics',
          );
          assert.equal(
            await page.locator('#nav [data-go="machines"]').getAttribute('href'),
            '/manage/machines',
          );
          if (width === 390) {
            await page.getByRole('button', { name: 'Open menu', exact: true }).tap();
            assert.equal(await page.locator('#lanes .srow').count(), workspaceChrome ? 1 : 0);
            await page.evaluate(() => {
              window.nativeDrawerClosed = new Promise((resolve) =>
                window.addEventListener('popstate', () => resolve(), { once: true }),
              );
            });
            await page.keyboard.press('Escape');
            await withDeadline(
              page.evaluate(() => window.nativeDrawerClosed),
              'native isolation drawer history close',
            );
            assert.equal(await page.evaluate(() => !!history.state?.drawer), false);
          }
          await page.locator('#page input').fill('Edited native task');
          await page.locator('#page input').press('End');
          await page.clock.runFor(8100);
          await assertNative();
          if (workspaceChrome) {
            await page
              .locator('#lanes')
              .getByText('Workspace Recent 2', { exact: true })
              .waitFor({
                state: width === 390 ? 'attached' : 'visible',
              });
            if (width === 390)
              assert.equal(
                await page.evaluate(() => document.body.classList.contains('drawer-open')),
                false,
                'the drawer stays closed while Recent refreshes',
              );
            assert.equal(publication, 2, 'native workspace chrome follows the existing cadence');
          }
          assert.equal(await page.locator('#page input').inputValue(), 'Edited native task');
          assert.equal(await page.evaluate(() => document.activeElement === nativeInput), true);
          await page.evaluate(() => {
            Object.defineProperty(document, 'visibilityState', {
              configurable: true,
              get: () => 'visible',
            });
            document.dispatchEvent(new Event('visibilitychange'));
            window.dispatchEvent(new Event('semon:refresh'));
          });
          await page.clock.runFor(8100);
          await assertNative();
          // Native links leave this document. A same-document history notification
          // cannot turn the native host into a source-selected reader either.
          for (const path of [
            '/sessions?machine=source-1',
            '/s/native/shared?machine=source-1',
            '/analytics',
            '/',
          ]) {
            await page.evaluate((path) => {
              history.pushState(null, '', path);
              dispatchEvent(new PopStateEvent('popstate'));
            }, path);
            await page.clock.runFor(32);
            await assertNative();
            assert.equal(await page.locator('#page input').inputValue(), 'Edited native task');
            assert.equal(await page.evaluate(() => document.activeElement === nativeInput), true);
          }
          await page.evaluate(() => {
            app.destroy();
            app.destroy();
          });
          assert.equal(await page.evaluate(() => nativeDestroys), 1);
          assert.equal(await page.locator('#page form').count(), 0);
          const readsAtDestroy = requests.length;
          await page.clock.runFor(8100);
          assert.equal(requests.length, readsAtDestroy, 'native chrome reads survived teardown');
          assert.deepEqual(errors, []);
        } finally {
          try {
            if (!page.isClosed()) await page.evaluate(() => window.app?.destroy());
            await withDeadline(
              page.unrouteAll({ behavior: 'wait' }),
              'native isolation routes drain',
            );
          } finally {
            await withDeadline(browser.close(), 'native isolation browser close');
          }
        }
      });

for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`workspace Recent stays separate from focused Machine publications at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch(),
        page = await browser.newPage({
          viewport: { width, height: 900 },
          colorScheme,
          isMobile: width === 390,
          hasTouch: width === 390,
        }),
        now = Date.parse('2026-10-07T12:00:00Z'),
        machines = Array.from({ length: 16 }, (_, i) => ({
          key: 'source-' + i,
          label: 'Machine ' + i,
          up: true,
          working: 0,
          sessions: 60,
          last: now - 1000,
        })),
        row = (machine, key, name, parent = null) => ({
          id: JSON.stringify([machine, key]),
          source_key: machine,
          catalog_key: key,
          session: {
            name,
            harness: 'claude',
            model: 'native-model',
            repo: null,
            state: 'idle',
            last: now - 1000,
            start: now - 2000,
            waiting_for: null,
            waiting_since: null,
            cost: 1,
            parent,
          },
        }),
        globalRecent = [
          row('source-0', 'shared', 'Alpha shared'),
          row('source-1', 'shared', 'Beta shared'),
          ...Array.from({ length: 58 }, (_, i) =>
            row('source-0', 'child-' + i, 'Published child ' + i, 'shared'),
          ),
        ],
        requests = [],
        errors = [];
      let workspaceReads = 0,
        workspaceMode = 'ready',
        stage = 'setup',
        primaryFailure;
      page.setDefaultTimeout(10000);
      // Keep product time paused while Node polls actual state with the existing
      // deadline; a readiness check must not depend on a frozen animation frame.
      const ready = async (label, predicate, arg) => {
        stage = label;
        let finished = false;
        try {
          await withDeadline(
            (async () => {
              while (!finished) {
                if (await page.evaluate(predicate, arg)) return;
                await new Promise((resolve) => setTimeout(resolve, 10));
              }
            })(),
            label,
          );
        } finally {
          finished = true;
        }
      };
      const publication = (machine, retired = false) => ({
        api: 1,
        version: (machine ? 'a' : retired ? 'c' : workspaceMode === 'partial' ? 'f' : 'd').repeat(
          64,
        ),
        now,
        next: { inbox: null, working: null, answered: null },
        totals: { waiting: machine ? 100 : retired ? 99 : 4, working: 0, answered: 0 },
        machines: machine ? machines.filter((item) => item.key === machine) : machines,
        recent: machine
          ? Array.from({ length: 60 }, (_, i) =>
              row(machine, i ? 'focused-' + i : 'shared', 'Focused ' + machine + ' row ' + i),
            )
          : retired
            ? [row('source-0', 'retired', 'Retired publication')]
            : globalRecent.map((item) =>
                workspaceMode === 'partial' && item.source_key === 'source-1'
                  ? { ...item, session: { ...item.session, name: 'Beta partial' } }
                  : item,
              ),
        inbox: Array.from({ length: machine ? 100 : retired ? 99 : 4 }, (_, i) => ({
          ...row(machine ?? 'source-0', 'waiting-' + i, 'Waiting ' + i),
          event: {
            id: 'question-' + i,
            at: now - 1000,
            done: null,
            ask: 'decision',
            status: 'wait',
            brief: 'Recorded decision',
            answer: null,
            declined: false,
          },
        })),
        working: [],
        answered: [],
        observations:
          !machine && workspaceMode === 'partial'
            ? [{ source_key: 'source-15', state: 'unavailable' }]
            : [],
        pending_observations:
          !machine && workspaceMode === 'pending'
            ? [{ source_key: 'source-15', catalog_key: 'waiting-for-publication' }]
            : [],
      });
      try {
        page.on('pageerror', (error) => errors.push(String(error)));
        await page.route('http://catalog.test/**', async (route) => {
          const url = new URL(route.request().url()),
            machine = url.searchParams.get('machine') ?? 'source-0';
          requests.push(url.pathname + url.search);
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
          if (url.pathname === '/viewer.css')
            return route.fulfill({ contentType: 'text/css', body: css });
          if (url.pathname === '/api/session-sources')
            return route.fulfill({
              json: {
                api: 1,
                items: machines.map((item) => ({ source_key: item.key, label: item.label })),
                next_cursor: null,
              },
            });
          if (url.pathname === '/api/session-capabilities') {
            // Capability work may finish after the compact Home response. Keep
            // this independently delayed so the paused clock cannot assume that
            // receiving Home has already scheduled the next refresh timer.
            if (workspaceMode !== 'ready') await new Promise((resolve) => setTimeout(resolve, 150));
            return route.fulfill({ json: { ...capabilities(true), source_key: machine } });
          }
          if (url.pathname === '/api/viewer/home') {
            const focused = url.searchParams.get('source');
            if (focused) return route.fulfill({ json: publication(focused) });
            const number = ++workspaceReads;
            if (workspaceMode === 'unavailable')
              return route.fulfill({
                status: 503,
                headers: { 'x-fixture-workspace-mode': workspaceMode },
                json: { error: 'overview_unavailable' },
              });
            return route.fulfill({
              headers: {
                'x-fixture-workspace-mode': workspaceMode,
                'x-fixture-publication': number === 1 ? 'retired' : 'current',
              },
              json: publication(null, number === 1),
            });
          }
          if (url.pathname === '/api/sessions') {
            const value = meta('shared');
            value.transcript = {
              generation: 'b'.repeat(64),
              errors_generation: 'b'.repeat(64),
              total: 3,
            };
            return route.fulfill({
              json: {
                ...list([value]),
                machine,
                machine_info: { key: machine, label: machine, freshness: 'cached' },
              },
            });
          }
          if (url.pathname === '/api/session-identity')
            return route.fulfill({
              json: { api: 1, identity: { ...identity('shared'), source_key: machine } },
            });
          if (url.pathname === '/api/session-transcript') {
            const value = transcript('shared', url.searchParams, 3);
            value.identity.source_key = machine;
            return route.fulfill({ json: value });
          }
          if (url.pathname === '/api/session-trace')
            return route.fulfill({
              json: {
                api: 1,
                source_key: machine,
                catalog_key: 'shared',
                root: 'trace-root',
                generation: 'e'.repeat(64),
                sessions: {
                  shared: {
                    ...globalRecent[1].session,
                    cost: { usd: 1, unpriced_models: [] },
                    tokens: [0, 0, 0],
                    busy: [],
                    wait_edges: [],
                    wait_edges_truncated: false,
                  },
                },
                participants: { 'unjoined-peer': 'Recorded peer' },
                participant_summaries: {
                  'unjoined-peer': {
                    name: 'Recorded peer',
                    harness: 'codex',
                    kind: 'Subagent',
                    model: 'native-model',
                    tokens: [1, 2, 0],
                    cost: { usd: 2, unpriced_models: [] },
                    observed_extent: {
                      scope: 'whole_session',
                      last: now - 1000,
                      status: { state: 'work', observation: 'current' },
                      recorded_activity: {
                        state: 'available',
                        kind: 'timestamp_clusters',
                        intervals: [
                          [now - 130000, now - 100000],
                          [now - 90000, now - 80000],
                        ],
                      },
                    },
                  },
                },
                handoffs: [
                  {
                    id: 'peer-relay',
                    kind: 'relay',
                    from: 'shared',
                    to: 'unjoined-peer',
                    at: now - 120000,
                    status: 'done',
                    brief: '',
                    declined: false,
                  },
                ],
                turns: [
                  {
                    id: 'trace-root',
                    sid: 'shared',
                    at: now - 600000,
                    u: true,
                    sent: ['peer-relay'],
                    end: { st: 'done', why: 'replied', at: now - 1000 },
                    last: true,
                  },
                ],
                frontier: [],
                previews: {},
                incomplete: true,
                incomplete_only_missing_receiving_turns: true,
                previews_incomplete: false,
              },
            });
          if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
          return route.fulfill({ contentType: 'text/html', body: html });
        });
        await page.goto('http://catalog.test/machines/source-0?machine=source-0');
        await page.clock.install({ time: new Date(now) });
        await page.clock.pauseAt(new Date(now + 1));
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          const nativeFetch = window.fetch.bind(window);
          let globalReads = 0;
          window.workspaceAborts = [];
          window.fetch = async (input, options = {}) => {
            const url = new URL(typeof input === 'string' ? input : input.url, location.href);
            if (url.pathname !== '/api/viewer/home' || url.searchParams.has('source'))
              return nativeFetch(input, options);
            const number = ++globalReads;
            options.signal.addEventListener('abort', () => workspaceAborts.push(number), {
              once: true,
            });
            if (number === 1) {
              window.workspaceNetworkPending = true;
              return new Promise((_, reject) => {
                options.signal.addEventListener(
                  'abort',
                  () => reject(new DOMException('Retired fixture request', 'AbortError')),
                  { once: true },
                );
              });
            }
            const response = await nativeFetch(input, options);
            if (number === 2) {
              const value = await response.json();
              window.workspaceBodyDelivered = true;
              response.json = async () => {
                window.workspaceBodyPending = true;
                await new Promise((resolve) => (window.releaseWorkspaceBody = resolve));
                window.workspaceBodyReturned = true;
                return value;
              };
            } else if (number === 3) {
              const value = await response.json();
              response.json = async () => {
                window.workspaceCurrentBodyPending = true;
                await new Promise((resolve) => (window.releaseCurrentWorkspaceBody = resolve));
                return value;
              };
            }
            return response;
          };
          window.app = CatalogViewer.mountViewerApplication({ machinesPath: '/machines' });
        });
        await ready(
          'initial focused Machine rows',
          () => document.querySelectorAll('#page .nrow').length === 60,
        );
        assert.match(await page.locator('#page .nrow').first().innerText(), /Focused source-0/);
        await ready('first workspace network gate', () => window.workspaceNetworkPending);
        // The exact bare Home request belongs to the existing navigation abort owner.
        await page.evaluate(() => document.querySelector('#nav [data-go="machines"]').click());
        await ready('delivered retired workspace body gate', () => window.workspaceBodyPending);
        assert.equal(await page.evaluate(() => workspaceAborts.includes(1)), true);
        assert.equal(await page.evaluate(() => window.workspaceBodyDelivered), true);
        // Already delivered JSON can still finish after cancellation: its epoch must reject it.
        await page.evaluate(() => {
          history.pushState(null, '', '/machines/source-1?machine=source-1');
          dispatchEvent(new PopStateEvent('popstate'));
        });
        await ready('retired delivered body cancellation', () => workspaceAborts.includes(2));
        await ready('fresh workspace body gate', () => window.workspaceCurrentBodyPending);
        await page.evaluate(() => window.releaseWorkspaceBody());
        await ready('retired delivered body return', () => window.workspaceBodyReturned);
        await page.clock.runFor(32);
        assert.doesNotMatch(await page.locator('#lanes').textContent(), /Retired publication/);
        assert.equal(
          (await page.locator('#nav [data-go="home"]').textContent()).replace(/\s/g, ''),
          'Home',
          'retired waiting totals cannot publish while the current workspace body is held',
        );
        await page.evaluate(() => window.releaseCurrentWorkspaceBody());
        await ready(
          'second focused Machine rows',
          () =>
            document.querySelectorAll('#page .nrow').length === 60 &&
            document.querySelector('#page .nrow')?.textContent.includes('Focused source-1'),
        );
        await ready('current workspace Recent publication', () =>
          document.querySelector('#lanes')?.textContent.includes('Alpha shared'),
        );
        assert.match(await page.locator('#lanes').textContent(), /Beta shared/);
        assert.equal((await page.locator('#lanes .row-machine').count()) > 1, true);
        const homeText = () => page.locator('#nav [data-go="home"]').textContent();
        assert.equal((await homeText()).replace(/\s/g, ''), 'Home4');
        assert.doesNotMatch(await page.locator('#lanes').textContent(), /Retired publication/);
        assert.equal((await homeText()).replace(/\s/g, ''), 'Home4');
        assert.match(await page.locator('#page .nrow').first().innerText(), /Focused source-1/);

        stage = 'open focused session';
        await withDeadline(page.locator('#page .nrow').first().click(), stage);
        stage = 'selected transcript';
        await withDeadline(page.locator('#page [data-entry-key="shared:0"]').waitFor(), stage);
        const current = page.locator('#lanes .srow[aria-current="page"]');
        assert.equal(await current.count(), 1);
        assert.equal(await current.getAttribute('data-id'), JSON.stringify(['source-1', 'shared']));
        assert.match(await page.locator('#lanes').textContent(), /Alpha shared/);
        assert.match(await page.locator('#lanes').textContent(), /Beta shared/);

        for (const mode of ['unavailable', 'partial', 'pending', 'ready']) {
          workspaceMode = mode;
          let delivered = false;
          const response = page
            .waitForResponse((value) => value.headers()['x-fixture-workspace-mode'] === mode)
            .then((receipt) => {
              delivered = true;
              return receipt;
            });
          stage = 'workspace response after ' + mode;
          // The refresh cadence is armed after asynchronous capability/name
          // reads finish. Advance the paused clock while allowing those real
          // network callbacks to settle; one 8.1s jump can strand its next timer.
          // Keep the existing response deadline and every scope/content check.
          for (let tick = 0; tick < 24 && !delivered; tick++) {
            await page.clock.runFor(1000);
            await Promise.race([response, new Promise((resolve) => setTimeout(resolve, 100))]);
          }
          const receipt = await withDeadline(response, stage);
          assert.equal(receipt.status(), mode === 'unavailable' ? 503 : 200);
          // Error reads intentionally release their unread body; the exact HTTP
          // receipt and resulting chrome state are this fixture's witnesses.
          if (mode !== 'unavailable') await withDeadline(receipt.finished(), stage + ' body');
          await ready(
            'workspace scope after ' + mode,
            (ready) =>
              ready
                ? !document.querySelector('#lanes')?.hasAttribute('aria-description')
                : document
                    .querySelector('#lanes')
                    ?.getAttribute('aria-description')
                    ?.includes('unavailable'),
            mode === 'ready',
          );
          assert.equal((await homeText()).replace(/\s/g, ''), mode === 'ready' ? 'Home4' : 'Home');
          await ready(
            'workspace rows after ' + mode,
            (label) => document.querySelector('#lanes')?.textContent.includes(label),
            mode === 'partial' ? 'Beta partial' : 'Beta shared',
          );
        }
        await page.evaluate(() => {
          history.pushState(null, '', '/trace/native/shared/trace-root?machine=source-1');
          dispatchEvent(new PopStateEvent('popstate'));
        });
        stage = 'Trace publication';
        await withDeadline(page.locator('#page .agents-panel').waitFor(), stage);
        const peer = page.locator('#page .agent-peer');
        assert.equal(await peer.getAttribute('data-depth'), '0');
        assert.equal(await peer.locator('.agent-harness-dot.h-codex').count(), 1);
        assert.equal(await peer.locator('.agent-relay-icon').count(), 1);
        assert.ok((await peer.locator('.agent-track .agent-gridline').count()) > 0);
        assert.equal(await peer.locator('.agent-segment.busy').count(), 2);
        assert.equal(
          await peer.locator('.agent-segment.idle,.agent-wait,.agent-spawn-tick').count(),
          0,
        );
        assert.match(
          await peer.locator('.agent-track').getAttribute('data-tip'),
          /whole-session timestamp clusters.*not continuous CPU activity/,
        );
        assert.equal(await peer.getAttribute('data-agent-id'), null);
        assert.equal(await peer.getAttribute('data-critical'), null);
        assert.match(await peer.getAttribute('aria-label'), /receiving|turn isn't in these logs/);
        assert.match(
          await peer.locator('.agent-duration').getAttribute('data-tip'),
          /Whole-session/,
        );
        assert.match(
          await page.locator('#page .flow').textContent(),
          /Its turn isn't in these logs/,
        );
        assert.doesNotMatch(
          await page.locator('#page .trace-summary').textContent(),
          /context is unavailable/,
        );
        assert.equal(await page.locator('#page .agent-edges path').count(), 0);
        assert.equal((await homeText()).replace(/\s/g, ''), 'Home4');
        assert.match(await page.locator('#lanes').textContent(), /Alpha shared/);
        assert.match(await page.locator('#lanes').textContent(), /Beta shared/);
        assert.equal(
          requests.some((path) => path.startsWith('/api/model')),
          false,
        );
        assert.equal(
          requests.some((path) => path.startsWith('/api/session-home')),
          false,
        );
        assert.equal(
          requests.some((path) => path.startsWith('/api/session-observe')),
          false,
        );
        assert.deepEqual(errors, []);
        await page.evaluate(() => window.app.destroy());
      } catch (error) {
        primaryFailure = error;
        let snapshot;
        try {
          snapshot = await withDeadline(
            page.evaluate(() => ({
              pathname: location.pathname,
              search: location.search,
              page: document.querySelector('#page')?.textContent.slice(0, 4000),
              recent: document.querySelector('#lanes')?.textContent.slice(0, 4000),
              nav: document.querySelector('#nav')?.textContent.slice(0, 1000),
              networkPending: window.workspaceNetworkPending,
              bodyDelivered: window.workspaceBodyDelivered,
              bodyPending: window.workspaceBodyPending,
              bodyReturned: window.workspaceBodyReturned,
              currentBodyPending: window.workspaceCurrentBodyPending,
              aborts: window.workspaceAborts?.slice(-16),
            })),
            'workspace failure diagnostic',
          );
        } catch (diagnosticError) {
          snapshot = { diagnosticError: String(diagnosticError).slice(0, 1000) };
        }
        console.error(
          'workspace-publication failure ' +
            JSON.stringify({
              width,
              colorScheme,
              stage,
              workspaceReads,
              workspaceMode,
              requests: requests.slice(-40).map((value) => value.slice(0, 512)),
              errors: errors.slice(-20).map((value) => value.slice(0, 1000)),
              snapshot,
            }),
        );
        throw error;
      } finally {
        let teardownFailure;
        try {
          if (!page.isClosed())
            await withDeadline(
              page.evaluate(() => {
                window.releaseWorkspaceBody?.();
                window.releaseCurrentWorkspaceBody?.();
                window.app?.destroy();
              }),
              'workspace fixture release',
            );
          await withDeadline(page.unrouteAll({ behavior: 'wait' }), 'workspace owned routes drain');
        } catch (teardownError) {
          teardownFailure = teardownError;
        } finally {
          try {
            await withDeadline(browser.close(), 'workspace browser close');
          } catch (closeError) {
            teardownFailure = teardownFailure
              ? new AggregateError(
                  [teardownFailure, closeError],
                  'Workspace teardown and close failed',
                )
              : closeError;
          }
        }
        if (teardownFailure) {
          if (primaryFailure)
            throw new AggregateError(
              [primaryFailure, teardownFailure],
              'Workspace assertion and teardown failed',
            );
          throw teardownFailure;
        }
      }
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
      version: 6,
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
    test(`Codex last-turn menu and tooltip observations at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        for (const [phase, state, outcome] of [
          ['completed', 'idle', 'Completed'],
          ['aborted', 'idle', 'Aborted'],
          ['errored', 'err', 'Errored'],
          ['active', 'work', 'Terminal not recorded'],
          [null, 'off', 'Unknown'],
        ]) {
          const page = await browser.newPage({
            viewport: { width, height: 900 },
            colorScheme,
            isMobile: width === 390,
            hasTouch: width === 390,
          });
          const nativeSource = {
            ...source('turn'),
            root: 'codex',
            path: 'sessions/2026/10/22/rollout-native-turn.jsonl',
          };
          const references = [{ source: nativeSource, state: 'cached' }];
          const details = {
            state,
            turns: 1,
            calls: 0,
            errors: 0,
            tool_calls: {},
            signals: {},
            cost: { usd: null },
            copied_usage: false,
            copilot: null,
            reported_runs: [],
            cost_check: [],
            live_observation: true,
            turn_lifecycle: phase && {
              phase,
              thread_id: 'PRIVATE-THREAD',
              turn_id: 'PRIVATE-TURN',
              offset: 987654321,
              source_generation: 'PRIVATE-GENERATION',
            },
          };
          const selected = {
            ...meta('turn'),
            name: 'Codex lifecycle fixture',
            harness: 'codex',
            kind: 'codex-run',
            source_refs: references,
            details,
          };
          const nativeIdentity = {
            ...identity('turn'),
            harness: 'codex',
            source_refs: references,
            read_scope: 'current',
          };
          await page.route('http://catalog.test/**', async (route) => {
            const url = new URL(route.request().url());
            const machine = url.searchParams.get('machine') ?? 'source';
            if (url.pathname === '/api/session-capabilities')
              return route.fulfill({ json: { ...capabilities(true), source_key: machine } });
            if (url.pathname === '/api/sessions')
              return route.fulfill({
                json: {
                  ...list([selected]),
                  machine,
                  machine_info: { key: machine, label: 'Fixture source', freshness: 'cached' },
                },
              });
            if (url.pathname === '/api/session-identity')
              return route.fulfill({
                json: { api: 1, identity: { ...nativeIdentity, source_key: machine } },
              });
            if (url.pathname === '/api/session-transcript') {
              const value = transcript('turn', url.searchParams, 1);
              value.session = selected;
              value.identity = {
                ...nativeIdentity,
                source_key: machine,
                read_scope: 'retained_history',
              };
              value.entries[0].provenance.source = nativeSource;
              return route.fulfill({ json: value });
            }
            if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
            return fulfillCatalogStatic(route, url);
          });
          await page.goto('http://catalog.test/sessions');
          await page.addScriptTag({ content: outputFiles[0].text });
          await page.evaluate(() => (window.app = CatalogViewer.mountViewerApplication()));
          await page.locator('#page [data-id="turn"]').click();
          await page.locator('#page [data-entry-key="turn:0"]').waitFor();
          assert.doesNotMatch(await page.locator('body').innerText(), /invalid history view/i);
          const tooltip = page.locator('#topbar [data-tip*="Last turn:"]:visible').first();
          assert.match(await tooltip.getAttribute('data-tip'), new RegExp(outcome));
          if (width === 390) await tooltip.tap();
          else await tooltip.hover();
          await page.getByRole('tooltip').waitFor({ state: 'visible' });
          assert.match(await page.getByRole('tooltip').innerText(), new RegExp(outcome));
          if (process.env.SEMON_CATALOG_OUT) {
            await mkdir(process.env.SEMON_CATALOG_OUT, { recursive: true });
            await page.screenshot({
              path: `${process.env.SEMON_CATALOG_OUT}/codex-${phase ?? 'unknown'}-${width}-${colorScheme}-tooltip.png`,
            });
          }
          await page
            .getByRole('button', { name: 'Session menu: details, cost and actions' })
            .click();
          const last = page
            .locator('dt')
            .filter({ hasText: /^Last turn$/ })
            .locator('xpath=following-sibling::dd[1]');
          await last.waitFor();
          assert.equal(await last.innerText(), outcome);
          assert.doesNotMatch(await page.locator('body').innerText(), /PRIVATE-|987654321/);
          if (process.env.SEMON_CATALOG_OUT)
            await page.screenshot({
              path: `${process.env.SEMON_CATALOG_OUT}/codex-${phase ?? 'unknown'}-${width}-${colorScheme}-menu.png`,
            });
          await page.close();
        }
      } finally {
        await browser.close();
      }
    });
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
                projection: { version: 6, generation: nativeGeneration.repeat(64) },
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
        // Finish the opening tail transaction before measuring reader-owned
        // geometry. The append below must preserve that settled anchor.
        await page.clock.runFor(2100);
        await page
          .locator('#page [data-entry-key="one:0"]')
          .evaluate((node) => node.scrollIntoView({ block: 'start' }));
        await page.clock.runFor(50);
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
        assert.ok(
          Math.abs(anchorBefore - anchorAfter) < 2,
          JSON.stringify({ anchorBefore, anchorAfter }),
        );
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
  let releaseFirstRange,
    page,
    capabilityReads = 0,
    ready = false,
    rangeReads = 0,
    stage = 'create fixture';
  const requests = [];
  const firstRangeHeld = new Promise((resolve) => {
      releaseFirstRange = resolve;
    }),
    firstRangeStarted = Promise.withResolvers();
  try {
    page = await browser.newPage();
    await page.route('http://catalog.test/**', async (route) => {
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
        if (++rangeReads === 1) {
          firstRangeStarted.resolve();
          await firstRangeHeld;
          return route.fulfill({ status: 503, body: '' });
        }
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
    stage = 'observe unavailable boot';
    await page
      .getByText('The source is updating or temporarily unavailable.', { exact: false })
      .waitFor();
    stage = 'recover boot and select pending projection';
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
    stage = 'refresh pending projection';
    await page.keyboard.press('Escape');
    ready = true;
    await page.clock.runFor(8100);
    await withDeadline(firstRangeStarted.promise, 'first unavailable selected range request');
    assert.equal(rangeReads, 1);
    // Deliver the temporary error while the product clock is paused. Advancing
    // across the recheck and retry together can otherwise miss this transient UI.
    const unavailableResponse = withDeadline(
      page.waitForResponse((response) => {
        const url = new URL(response.url());
        return (
          url.pathname === '/api/session-transcript' &&
          url.searchParams.get('machine') === 'source' &&
          url.searchParams.get('sid') === 'one' &&
          response.request().method() === 'GET'
        );
      }),
      'first unavailable selected range response headers',
    );
    stage = 'deliver unavailable selected range';
    releaseFirstRange();
    assert.equal((await unavailableResponse).status(), 503);
    stage = 'observe unavailable selected range';
    await page
      .getByText('The source is updating or temporarily unavailable.', { exact: false })
      .waitFor();
    stage = 'recover selected range after original retry interval';
    await page.clock.runFor(1100);
    await page.locator('#page [data-entry-key="one:5"]').waitFor();
    assert.equal(await page.getByRole('button', { name: 'Open Composer' }).count(), 0);
    await page
      .getByRole('region', { name: 'Environment status' })
      .getByText('This session has ended.', { exact: true })
      .waitFor();
    assert.equal(requests.includes('/api/model'), false);
    stage = 'destroy fixture';
    await page.evaluate(() => app.destroy());
    const afterDestroy = requests.length;
    await page.clock.runFor(30000);
    assert.equal(requests.length, afterDestroy);
    assert.equal(await page.evaluate(() => runtimeStreams.every((stream) => stream.closed)), true);
  } catch (error) {
    let dom;
    try {
      dom = await withDeadline(
        page.evaluate(() => {
          const known = [
            'The source is updating or temporarily unavailable.',
            'Current native session selection is unavailable.',
            'Loading',
            'No native records',
            'Cached history',
            'Recorded history',
          ];
          return {
            clock: Date.now(),
            path: location.pathname,
            historyKeys: Object.keys(history.state ?? {}).slice(0, 12),
            entries: document.querySelectorAll('#page [data-entry-key]').length,
            notes: [...document.querySelectorAll('#page [role="status"], #page .catalog-note')]
              .slice(0, 12)
              .map((node) => {
                const text = node.textContent.trim();
                return {
                  known: known.find((prefix) => text.startsWith(prefix)) ?? null,
                  length: text.length,
                  visible: node.getBoundingClientRect().height > 0,
                };
              }),
          };
        }),
        'unavailable projection failure evidence',
      );
    } catch {
      dom = { diagnosticUnavailable: true };
    }
    console.error(
      'unavailable projection fixture failure: ' +
        JSON.stringify({
          stage,
          capabilityReads,
          rangeReads,
          ready,
          requests: requests.slice(-20),
          dom,
        }),
    );
    throw error;
  } finally {
    releaseFirstRange();
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
        if (url.pathname === '/api/viewer/home')
          return route.fulfill({
            json: {
              api: 1,
              version: 'a'.repeat(64),
              now: 10000,
              next: { inbox: null, working: null, answered: null },
              totals: { waiting: 2, working: 0, answered: 0 },
              machines: [
                {
                  key: 'one',
                  label: 'Same hostname',
                  up: true,
                  working: 0,
                  sessions: 1,
                  last: 10000,
                },
              ],
              recent: [
                {
                  id: 'chooser-recent',
                  source_key: 'one',
                  catalog_key: 'chooser-recent',
                  session: {
                    name: 'Source picker Recent',
                    harness: 'claude',
                    model: '',
                    repo: null,
                    state: 'idle',
                    last: 10000,
                    start: 9000,
                    content_generation: 'b'.repeat(64),
                  },
                },
              ],
              inbox: [],
              working: [],
              answered: [],
              observations: [],
              pending_observations: [{ source_key: 'one', catalog_key: 'pending' }],
            },
          });
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
      await page.goto('http://catalog.test/sessions?choose_source=1');
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
      await page
        .locator('#lanes')
        .getByText('Source picker Recent', { exact: true })
        .waitFor({ state: 'attached' });
      assert.equal(requests.filter((path) => path.startsWith('/api/viewer/home')).length, 1);
      assert.equal(requests.includes('/api/session-capabilities'), false);
      assert.equal(
        requests.some((path) =>
          /^\/api\/(sessions|session-(observe|transcript|home|trace|fields|lookup|locate))(\?|$)/.test(
            path,
          ),
        ),
        false,
      );
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

for (const { width, colorScheme } of [
  { width: 390, colorScheme: 'light' },
  { width: 390, colorScheme: 'dark' },
  { width: 1280, colorScheme: 'light' },
  { width: 1280, colorScheme: 'dark' },
])
  test(`search submission and Back retain list context at ${width}px ${colorScheme}`, async () => {
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({
        viewport: { width, height: width === 390 ? 480 : 860 },
        isMobile: width === 390,
        hasTouch: width === 390,
      });
      await page.emulateMedia({ colorScheme });
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
          const params = Object.fromEntries(u.searchParams),
            term = u.searchParams.get('q');
          reads.push(params);
          if (fail) return route.fulfill({ status: 503, json: { error: 'unavailable' } });
          const result = list(term === 'missing' ? [] : items);
          if (term) {
            result.search = {
              semantics: 'unicode_lowercase_substring',
              fields: ['name', 'key', 'repo', 'branch', 'model', 'harness'],
              partial: term === 'Session',
              candidates: 36,
              index_complete: term !== 'Session',
              candidate_budget: 512,
              byte_budget: 2097152,
            };
            result.freshness =
              term === 'Session' ? 'stale' : term === 'missing' ? 'unavailable' : 'cached';
          }
          if (term === 'Session') result.next_cursor = 'next-page';
          return route.fulfill({
            json: result,
          });
        }
        if (u.pathname === '/api/session-identity')
          return route.fulfill({
            json: { api: 1, identity: identity(u.searchParams.get('sid')) },
          });
        if (u.pathname === '/api/session-transcript')
          return route.fulfill({
            json: transcript(u.searchParams.get('sid'), u.searchParams, 10),
          });
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
      if (process.env.SEMON_UI_OUT) {
        await mkdir(process.env.SEMON_UI_OUT, { recursive: true });
        await page.evaluate(() => document.fonts.ready);
        await page.screenshot({
          path: `${process.env.SEMON_UI_OUT}/sessions-filter-placement-list-${width}-${colorScheme}.png`,
        });
      }
      assert.equal(await page.locator('.catalog-result-header').count(), 0);
      const summary = page.locator('.ph .sub span').first();
      assert.match(await summary.innerText(), /36/);
      assert.match(await summary.innerText(), /sessions/);
      assert.doesNotMatch(await summary.innerText(), /loaded/);
      assert.match(
        await summary.getAttribute('aria-description'),
        /Selected source: Fixture source/,
      );
      assert.match(await summary.getAttribute('data-tip'), /Cached history/);
      const searchQualification = await query.getAttribute('aria-description');
      assert.match(searchQualification, /Metadata search checks session names, IDs/);
      assert.match(
        searchQualification,
        /Conversation text and history across sources are unavailable/,
      );
      assert.equal(await query.getAttribute('data-tip'), searchQualification);
      // Filter choices commit together when the existing shared sheet closes.
      await page.locator('.facet-btn').click();
      const filterBody = page.locator('.filters-sheet .vb');
      await filterBody.waitFor();
      assert.equal(await page.locator('.filters-sheet .facet-info').count(), 0);
      assert.doesNotMatch(
        await filterBody.innerText(),
        /Selected source:|Cached history|Metadata search checks|Ordered by latest recorded activity/,
      );
      if (width === 390) {
        assert.equal(
          await filterBody.evaluate((body) => body.scrollHeight > body.clientHeight + 1),
          false,
          'compact phone Filter fits without the removed paragraph',
        );
        // Stress the existing scroll owner, then restore the original capture viewport.
        await page.setViewportSize({ width, height: 320 });
        const scrollable = await filterBody.evaluate(
          (body) => body.scrollHeight > body.clientHeight + 1,
        );
        assert.equal(scrollable, true, 'phone Filter fields share the scrollable .vb owner');
        assert.equal(
          await page.evaluate(() => {
            const event = new Event('touchmove', { bubbles: true, cancelable: true });
            document.querySelector('.filters-sheet .sh-select').dispatchEvent(event);
            return event.defaultPrevented;
          }),
          false,
          'touch scrolling is allowed from the Filter fields inside .vb',
        );
        assert.equal(
          await page.evaluate(() => {
            const event = new Event('touchmove', { bubbles: true, cancelable: true });
            document.querySelector('.filters-sheet .vh').dispatchEvent(event);
            return event.defaultPrevented;
          }),
          true,
          'touch scrolling remains held outside the Filter scroll owner',
        );
        const bodyBox = await filterBody.boundingBox();
        assert.ok(bodyBox);
        await page.mouse.move(bodyBox.x + 8, bodyBox.y + 8);
        await page.mouse.wheel(0, 180);
        await page.waitForFunction(
          () => document.querySelector('.filters-sheet .vb')?.scrollTop > 0,
        );
        assert.ok(await page.locator('.filters-sheet .vb').evaluate((node) => node.scrollTop > 0));
        await page.setViewportSize({ width, height: 480 });
        await page.waitForFunction(
          () => document.querySelector('.filters-sheet .vb')?.scrollTop === 0,
        );
      }
      if (process.env.SEMON_UI_OUT)
        await page.screenshot({
          path: `${process.env.SEMON_UI_OUT}/sessions-filter-placement-sheet-${width}-${colorScheme}.png`,
        });
      const machine = page.locator('.sh-select[data-label="Machine"]');
      assert.match(await machine.locator('.sh-select-trigger').innerText(), /All machines/);
      await machine.locator('.sh-select-trigger').click();
      const machineOptions = page.locator('[role="listbox"][aria-label="Machine"] [role="option"]');
      await machineOptions.first().waitFor();
      assert.equal(await machineOptions.count(), 1);
      assert.equal((await machineOptions.first().innerText()).trim(), 'All machines');
      await page.keyboard.press('Escape');
      await page.waitForFunction(() => !history.state?.shSelect);
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
      assert.equal(
        await page.locator('.facet-btn').evaluate((node) => document.activeElement === node),
        true,
      );
      await page.locator('.facet-btn').click();
      await page.goBack();
      await page.waitForFunction(() => !history.state?.sheet);
      await page.locator('.filters-sheet').waitFor({ state: 'hidden' });
      assert.equal(
        await page.locator('.facet-btn').evaluate((node) => document.activeElement === node),
        true,
      );
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
      assert.equal(reads[2].machine, 'source');
      await page
        .getByText(
          'Search checked a bounded part of the index. Load more sessions to continue looking for matches.',
        )
        .waitFor();
      await page
        .getByText('Search is incomplete while recorded sessions are being indexed.')
        .waitFor();
      const staleWarning = page
        .locator('.catalog-note[role="status"]')
        .filter({ hasText: 'History observation is stale' });
      await staleWarning.waitFor();
      for (const warning of [
        'Search checked a bounded part of the index. Load more sessions to continue looking for matches.',
        'Search is incomplete while recorded sessions are being indexed.',
        'History observation is stale',
      ]) {
        assert.equal(
          await page
            .locator('.catalog-note[role="status"]')
            .filter({ hasText: warning })
            .evaluate((node) => node.closest('.filters-sheet')),
          null,
        );
      }
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
      assert.equal(reads.at(-1).harness, undefined);
      assert.equal(reads.at(-1).repo, undefined);
      await query.fill('missing');
      await query.press('Enter');
      await page.getByText('No recorded sessions match these filters.').waitFor();
      const unavailableWarning = page
        .locator('.catalog-note[role="status"]')
        .filter({ hasText: 'History observation is unavailable' });
      await unavailableWarning.waitFor();
      assert.equal(
        await unavailableWarning.evaluate((node) => node.closest('.filters-sheet')),
        null,
      );
      assert.equal(
        await page.getByRole('button', { name: 'Clear filters', exact: true }).count(),
        1,
      );
      fail = true;
      await query.fill('Session');
      await query.press('Enter');
      await page.getByRole('button', { name: 'Retry', exact: true }).waitFor();
      assert.equal(
        await page
          .getByRole('button', { name: 'Retry', exact: true })
          .evaluate((node) => node.closest('.filters-sheet')),
        null,
      );
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
  for (const colorScheme of ['light', 'dark'])
    test(`source freshness notices stay outside Filter at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({ viewport: { width, height: 860 } });
        await page.emulateMedia({ colorScheme });
        await page.route('http://catalog.test/**', (route) => {
          const url = new URL(route.request().url());
          if (url.pathname === '/api/session-capabilities')
            return route.fulfill({
              json: {
                ...capabilities(true),
                metadata_search: true,
                filters: ['harness', 'repo', 'q'],
              },
            });
          if (url.pathname === '/api/sessions') {
            const term = url.searchParams.get('q'),
              result = list([meta('freshness')]);
            result.freshness =
              term === 'stale'
                ? 'stale'
                : term === 'updating'
                  ? 'updating'
                  : term === 'incomplete'
                    ? 'incomplete'
                    : term === 'unavailable'
                      ? 'unavailable'
                      : 'cached';
            if (term === 'updating') result.completeness = { state: 'partial' };
            if (term)
              result.search = {
                semantics: 'unicode_lowercase_substring',
                fields: ['name', 'key', 'repo', 'branch', 'model', 'harness'],
                partial: false,
                candidates: 1,
                index_complete: true,
                candidate_budget: 512,
                byte_budget: 2097152,
              };
            return route.fulfill({ json: result });
          }
          return route.fulfill({
            contentType: url.pathname === '/viewer.css' ? 'text/css' : 'text/html',
            body: url.pathname === '/viewer.css' ? css : html,
          });
        });
        await page.goto('http://catalog.test/sessions');
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => (window.app = CatalogViewer.mountViewerApplication()));
        const query = page.getByRole('searchbox', { name: 'Search session details', exact: true });
        await page.locator('#page [data-id="freshness"]').waitFor();
        for (const [term, warning] of [
          ['stale', 'History observation is stale'],
          ['incomplete', 'History is incomplete'],
          ['unavailable', 'History observation is unavailable'],
          ['updating', 'More sessions are being discovered. This list is incomplete.'],
        ]) {
          const response = page.waitForResponse((candidate) => {
            const url = new URL(candidate.url());
            return url.pathname === '/api/sessions' && url.searchParams.get('q') === term;
          });
          await query.fill(term);
          await query.press('Enter');
          await response;
          const notice = page.locator('.catalog-note[role="status"]').filter({ hasText: warning });
          await notice.waitFor();
          assert.equal(await notice.evaluate((node) => node.closest('.filters-sheet')), null);
          assert.equal(await page.locator('.catalog-note[role="status"]').count(), 1);
          assert.equal(await page.locator('.catalog-result-header').count(), 0);
          if (term === 'updating')
            assert.equal(
              await page.locator('.ph').getAttribute('data-catalog-observation'),
              'Discovering history',
            );
        }
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

for (const width of [820, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`local tool expansion keeps jump visibility through unchanged observations at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch(),
        page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme }),
        errors = [];
      let releaseMetadata,
        releaseIdentity,
        signalMetadata,
        signalIdentity,
        holdObservations = false,
        generation = 'b',
        total = 40,
        stage = 'setup',
        primaryError,
        errorOverflow = false;
      const metadataGate = new Promise((resolve) => {
          releaseMetadata = resolve;
        }),
        identityGate = new Promise((resolve) => {
          releaseIdentity = resolve;
        }),
        metadataStarted = new Promise((resolve) => {
          signalMetadata = resolve;
        }),
        identityStarted = new Promise((resolve) => {
          signalIdentity = resolve;
        });
      const waitForReading = (predicate, label, argument) => {
        let waiting = true;
        return withDeadline(
          (async () => {
            while (waiting && !(await page.evaluate(predicate, argument)))
              await new Promise((resolve) => setTimeout(resolve, 10));
          })(),
          label,
        ).finally(() => {
          waiting = false;
        });
      };
      try {
        page.setDefaultTimeout(10000);
        page.on('pageerror', (error) => {
          if (errors.length < 20) errors.push(String(error).slice(0, 1000));
          else errorOverflow = true;
        });
        await page.route('http://catalog.test/**', async (route) => {
          const url = new URL(route.request().url());
          if (url.pathname === '/api/session-capabilities')
            return route.fulfill({ json: capabilities(true) });
          if (url.pathname === '/api/sessions') {
            const selected = url.searchParams.get('sid') === 'one';
            if (selected && holdObservations) {
              signalMetadata();
              await metadataGate;
            }
            return route.fulfill({
              json: list([
                {
                  ...meta('one'),
                  name: selected && holdObservations ? 'Observed tool session' : 'Session one',
                  transcript: {
                    generation: generation.repeat(64),
                    errors_generation: generation.repeat(64),
                    total,
                  },
                },
              ]),
            });
          }
          if (url.pathname === '/api/session-identity') {
            if (holdObservations) {
              signalIdentity();
              await identityGate;
            }
            return route.fulfill({ json: { api: 1, identity: identity('one') } });
          }
          if (url.pathname === '/api/session-transcript') {
            if (
              url.searchParams.has('generation') &&
              url.searchParams.get('generation') !== generation.repeat(64)
            )
              return route.fulfill({ status: 409, json: { error: 'stale_projection' } });
            const value = transcript('one', url.searchParams, total, generation);
            value.entries = value.entries.map((entry) => {
              const { text, clipped, field, ...rest } = entry;
              return {
                ...rest,
                k: 'tool',
                tid: 'tool-' + entry.slot,
                name: 'Bash',
                arg: 'echo recorded-' + entry.slot,
                out: 'Recorded output',
                ok: true,
              };
            });
            return route.fulfill({ json: value });
          }
          if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 404, body: '' });
          return fulfillCatalogStatic(route, url);
        });
        await page.goto('http://catalog.test/sessions');
        await page.clock.install({ time: new Date('2026-10-18T00:00:00Z') });
        await page.clock.pauseAt(new Date('2026-10-18T00:00:00.001Z'));
        await page.evaluate(() => {
          window.readingScrollEvents = 0;
          window.identityJSONCompleted = 0;
          const fetch = window.fetch;
          window.fetch = async (...args) => {
            const response = await fetch(...args);
            if (new URL(response.url).pathname === '/api/session-identity') {
              const json = response.json.bind(response);
              response.json = async () => {
                const value = await json();
                window.identityJSONCompleted++;
                return value;
              };
            }
            return response;
          };
          document.querySelector('#main').addEventListener('scroll', () => {
            window.readingScrollEvents++;
          });
        });
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.app = CatalogViewer.mountViewerApplication();
        });
        await page.locator('#page [data-id="one"]').click();
        await page.locator('#page .tsum').waitFor();
        await waitForReading(() => window.identityJSONCompleted > 0, 'initial selected identity');
        await page.evaluate(() => document.fonts.ready);
        await page.clock.runFor(34);
        const geometry = () =>
          page.locator('#main').evaluate((main) => ({
            top: main.scrollTop,
            gap: main.scrollHeight - main.scrollTop - main.clientHeight,
            jump: !document.querySelector('#page .jump-wrap').hidden,
            scrollEvents: window.readingScrollEvents,
          }));
        assert.deepEqual(await geometry(), { top: 0, gap: 0, jump: false, scrollEvents: 0 });
        stage = 'expand at top';
        await page.locator('#page .tsum').click();
        await page.clock.runFor(34);
        const expanded = await geometry();
        assert.ok(expanded.gap > 80, JSON.stringify(expanded));
        assert.equal(expanded.top, 0);
        assert.equal(expanded.scrollEvents, 0);
        assert.equal(expanded.jump, false, 'Local expansion does not observe a scroll');
        stage = 'delayed metadata';
        holdObservations = true;
        await page.clock.runFor(3100);
        await withDeadline(metadataStarted, 'held metadata observation');
        const identityBefore = await page.evaluate(() => window.identityJSONCompleted);
        releaseMetadata();
        await page.locator('#page .ph > h1').filter({ hasText: 'Observed tool session' }).waitFor();
        await withDeadline(identityStarted, 'held unchanged identity');
        assert.deepEqual(await geometry(), expanded, 'Metadata redraw retains the pre-input dock');
        stage = 'delayed unchanged identity';
        releaseIdentity();
        await waitForReading(
          (before) => window.identityJSONCompleted > before,
          'unchanged identity JSON delivery',
          identityBefore,
        );
        // The next browser task runs after the accepted identity continuation.
        await page.evaluate(() => Promise.resolve());
        assert.deepEqual(await geometry(), expanded, 'Identity redraw retains geometry and dock');
        stage = 'new native projection';
        generation = 'c';
        total = 41;
        await page.clock.runFor(3100);
        await page.locator('#page [data-entry-key="one:40"]').waitFor({ state: 'attached' });
        const updated = await geometry();
        assert.equal(updated.top, 0, 'Native updates retain the expanded reading position');
        assert.equal(updated.jump, true, 'A changed native projection observes the new extent');
        stage = 'actual scroll';
        const point = await page.locator('#main').evaluate((main) => {
          const rect = main.getBoundingClientRect();
          return { x: rect.right - 20, y: rect.top + 200 };
        });
        await page.mouse.move(point.x, point.y);
        await page.mouse.wheel(0, 100000);
        await page.clock.runFor(34);
        await waitForReading(() => {
          const main = document.querySelector('#main');
          return (
            main.scrollTop > 0 &&
            main.scrollHeight - main.scrollTop - main.clientHeight <= 1 &&
            document.querySelector('#page .jump-wrap').hidden
          );
        }, 'actual scroll to bottom');
        await page.mouse.wheel(0, -100000);
        await page.clock.runFor(34);
        await waitForReading(
          () =>
            document.querySelector('#main').scrollTop === 0 &&
            !document.querySelector('#page .jump-wrap').hidden,
          'actual return scroll updates dock',
        );
        assert.ok((await geometry()).scrollEvents > 0);
        stage = 'deliberate jump';
        await page.getByRole('button', { name: /Jump to bottom/ }).click();
        await page.clock.runFor(34);
        await waitForReading(() => {
          const main = document.querySelector('#main');
          return (
            main.scrollHeight - main.scrollTop - main.clientHeight <= 1 &&
            document.querySelector('#page .jump-wrap').hidden
          );
        }, 'deliberate Jump to Bottom');
        assert.equal(errorOverflow, false);
        assert.deepEqual(errors, []);
      } catch (error) {
        primaryError = error;
        console.error(
          'reading dock diagnostic',
          JSON.stringify({
            width,
            colorScheme,
            stage,
            errors: errors.slice(-10),
            errorOverflow,
            state: await withDeadline(
              page.evaluate(() => {
                const main = document.querySelector('#main'),
                  jump = document.querySelector('#page .jump-wrap');
                return {
                  top: main?.scrollTop,
                  height: main?.scrollHeight,
                  viewport: main?.clientHeight,
                  jumpHidden: jump?.hidden,
                  scrollEvents: window.readingScrollEvents,
                  identityJSONCompleted: window.identityJSONCompleted,
                };
              }),
              'reading dock failure geometry',
            ).catch(() => null),
          }),
        );
        throw error;
      } finally {
        releaseMetadata();
        releaseIdentity();
        try {
          await withDeadline(browser.close(), 'reading dock browser teardown');
        } catch (error) {
          if (primaryError)
            throw new AggregateError([primaryError, error], 'Reading dock failure and teardown');
          throw error;
        }
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
            projection: { version: 6, generation: 'b'.repeat(64) },
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
    await page.waitForFunction(() =>
      document
        .querySelector('#page [data-entry-key="one:0"]')
        ?.textContent.includes('Anchor target record'),
    );
    // The accepted range schedules its anchor placement on the next frame.
    // Advance the paused clock before inspecting content-visibility text.
    await page.clock.runFor(34);
    assert.match(
      await page.locator('#page [data-entry-key="one:0"]').innerText(),
      /Anchor target record/,
    );
    assert.match(await field.textContent(), /Final field segment\./);
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

for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark'])
    for (const failure of ['observation', 'content'])
      test(`selected ${failure} recovers without a new projection at ${width}px ${colorScheme}`, async () => {
        const browser = await chromium.launch();
        try {
          const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme });
          page.setDefaultTimeout(10000);
          let transcriptReads = 0,
            observationReads = 0;
          const errors = [];
          page.on('pageerror', (error) => errors.push(String(error)));
          await page.route('http://catalog.test/**', async (route) => {
            const url = new URL(route.request().url());
            if (url.pathname === '/api/session-capabilities')
              return route.fulfill({ json: capabilities(true) });
            if (url.pathname === '/api/session-identity')
              return route.fulfill({ json: { api: 1, identity: identity('one') } });
            if (url.pathname === '/api/sessions') {
              if (url.searchParams.has('sid')) {
                observationReads++;
                if (failure === 'observation' && observationReads === 1)
                  return route.abort('failed');
              }
              return route.fulfill({
                json: list([
                  {
                    ...meta('one'),
                    transcript: {
                      generation: 'b'.repeat(64),
                      errors_generation: 'b'.repeat(64),
                      total: 65,
                    },
                  },
                ]),
              });
            }
            if (url.pathname === '/api/session-transcript') {
              const body = transcript('one', url.searchParams);
              transcriptReads++;
              if (failure === 'content' && transcriptReads === 1) {
                body.entries[0].text = '';
                body.entries[0].freshness = { state: 'incomplete' };
                body.freshness.state = 'incomplete';
              }
              return route.fulfill({ json: body });
            }
            return fulfillCatalogStatic(route, url);
          });
          await page.goto('http://catalog.test/sessions?machine=source');
          await page.clock.install({ time: new Date('2026-10-19T00:00:00Z') });
          await page.clock.pauseAt(new Date('2026-10-19T00:00:00.001Z'));
          await page.addScriptTag({ content: outputFiles[0].text });
          await page.evaluate(() => {
            window.app = CatalogViewer.mountViewerApplication({
              machinesPath: '/machines',
              loadMachines: async () => {
                throw Error('No global native inventory');
              },
            });
          });
          await page.locator('#page [data-id="one"]').click();
          await page.locator('#page [data-entry-key="one:5"]').waitFor({ state: 'attached' });
          if (failure === 'observation')
            await page
              .getByText('Source observation is unavailable. Retrying…', { exact: true })
              .waitFor({ state: 'attached' });
          await page.clock.runFor(11000);
          if (failure === 'content') {
            await page
              .locator('#page [data-entry-key="one:5"]')
              .filter({ hasText: 'Original one record 5' })
              .waitFor();
            assert.ok(transcriptReads >= 2, 'incomplete native bytes were never reobserved');
          } else {
            await page
              .getByText('Source observation is unavailable. Retrying…', { exact: true })
              .waitFor({ state: 'detached' });
            assert.ok(observationReads >= 2, 'no successful source reobservation');
            assert.equal(transcriptReads, 1, 'metadata recovery reread unchanged native content');
          }
          assert.deepEqual(errors, []);
          await page.evaluate(() => app.destroy());
        } finally {
          await browser.close();
        }
      });

for (const width of [390, 1280])
  test(`host pages share the reader router and reject stale content at ${width}px`, async () => {
    const browser = await chromium.launch();
    const page = await browser.newPage({ viewport: { width, height: 900 } });
    const errors = [];
    page.on('pageerror', (error) => errors.push(error.message));
    try {
      await page.route('http://catalog.test/**', async (route) => {
        const url = new URL(route.request().url());
        if (url.pathname === '/api/session-capabilities')
          return route.fulfill({ json: capabilities(true) });
        if (url.pathname === '/api/sessions') return route.fulfill({ json: list([meta('one')]) });
        if (url.pathname === '/api/session-transcript')
          return route.fulfill({ json: transcript('one', url.searchParams) });
        if (url.pathname.startsWith('/api/')) return route.fulfill({ status: 503, json: {} });
        return fulfillCatalogStatic(route, url);
      });
      await page.goto('http://catalog.test/machines');
      await page.addScriptTag({ content: outputFiles[0].text });
      await page.evaluate(() => {
        window.hostMounted = 0;
        window.hostDestroyed = 0;
        window.hostAborted = false;
        const content = (name) => {
          const element = document.createElement('section');
          const heading = document.createElement('h1');
          heading.textContent = name;
          const form = document.createElement('form');
          form.method = 'post';
          form.action = '/machines/rename';
          const input = document.createElement('input');
          input.name = 'name';
          input.value = 'Retained machine';
          form.append(input);
          element.append(heading, form);
          return {
            element,
            title: name,
            nav: 'machines',
            mount() {
              window.hostMounted++;
            },
            destroy() {
              window.hostDestroyed++;
              element.remove();
            },
          };
        };
        window.app = CatalogViewer.mountViewerApplication({
          machinesPath: '/machines',
          pages: {
            match: (url) =>
              ['/machines', '/account'].includes(url.pathname)
                ? { title: url.pathname === '/machines' ? 'Machines' : 'Account', nav: 'machines' }
                : null,
            initial: content('Machines'),
            load(url, signal) {
              if (window.failHost) return Promise.reject({ status: 503 });
              if (window.holdHost)
                return new Promise((resolve) => {
                  window.releaseHost = () => resolve(content('Stale account'));
                  signal.addEventListener('abort', () => {
                    window.hostAborted = true;
                  });
                });
              return Promise.resolve(
                content(url.pathname === '/machines' ? 'Machines' : 'Account'),
              );
            },
          },
          account: {
            name: 'Hosted account',
            login: 'hosted',
            initials: 'HA',
            workspaces: [],
            links: [
              { label: 'Account', href: '/account', method: 'get', danger: false },
              { label: 'Sessions', href: '/sessions', method: 'get', danger: false },
            ],
          },
        });
        window.sharedApp = document.querySelector('.app');
        window.sharedSidebar = document.querySelector('#sidebar');
      });
      await page.getByRole('heading', { name: 'Machines', exact: true }).waitFor();
      assert.equal(await page.locator('form').getAttribute('method'), 'post');
      const openAccount = async () => {
        if (
          width === 390 &&
          !(await page.locator('body').evaluate((node) => node.classList.contains('drawer-open')))
        )
          await page.locator('#lead-btn').click();
        await page
          .locator(width === 390 ? '#account-drawer .account-trigger' : '#topbar .account-trigger')
          .click();
      };
      await openAccount();
      // Modified clicks must reach the browser without changing the application
      // route or consuming the phone sheet's history entry.
      for (const modifier of ['altKey', 'ctrlKey', 'metaKey', 'shiftKey']) {
        assert.equal(
          await page.evaluate((modifier) => {
            let intercepted;
            document.addEventListener(
              'click',
              (event) => {
                intercepted = event.defaultPrevented;
                event.preventDefault();
              },
              { once: true },
            );
            document.querySelector('.account-popover a[href="/account"]').dispatchEvent(
              new MouseEvent('click', {
                bubbles: true,
                cancelable: true,
                button: 0,
                [modifier]: true,
              }),
            );
            return intercepted;
          }, modifier),
          false,
          modifier + ': account menu consumed a modified click',
        );
      }
      await page.locator('.account-popover a[href="/account"]').click();
      await page.getByRole('heading', { name: 'Account', exact: true }).waitFor();
      assert.deepEqual(
        await page.evaluate(() => ({
          app: sharedApp === document.querySelector('.app'),
          sidebar: sharedSidebar === document.querySelector('#sidebar'),
          sheet: !!history.state?.sheet,
          drawer: document.body.classList.contains('drawer-open'),
          menu: !!document.querySelector('.account-popover'),
        })),
        { app: true, sidebar: true, sheet: false, drawer: false, menu: false },
        'account menu navigation replaced the document or retained an overlay',
      );
      await page.goBack();
      await page.getByRole('heading', { name: 'Machines', exact: true }).waitFor();
      assert.equal(await page.evaluate(() => !!history.state?.sheet), false);
      await openAccount();
      await page.locator('.account-popover a[href="/sessions"]').click();
      await page.locator('#page [data-session-row="list"]').waitFor();
      assert.equal(
        await page.evaluate(() => sharedApp === document.querySelector('.app')),
        true,
        'account menu reader link replaced the application document',
      );
      await page.goBack();
      await page.getByRole('heading', { name: 'Machines', exact: true }).waitFor();
      const navigate = async (key) => {
        if (
          width === 390 &&
          !(await page.locator('body').evaluate((node) => node.classList.contains('drawer-open')))
        )
          await page.locator('#lead-btn').click();
        await page.locator(`#nav [data-go="${key}"]`).click();
      };
      await navigate('sessions');
      await page.locator('#page [data-session-row="list"]').waitFor();
      await page.locator('#page [data-session-row="list"]').click();
      await page.locator('#page [data-entry-key]').first().waitFor();
      assert.equal(new URL(page.url()).searchParams.get('machine'), 'source');
      await navigate('machines');
      await page.getByRole('heading', { name: 'Machines', exact: true }).waitFor();
      await page.waitForTimeout(50);
      assert.equal(
        new URL(page.url()).pathname,
        '/machines',
        'closing the drawer rolled back the route',
      );
      await page.goBack();
      await page.locator('#page [data-entry-key]').first().waitFor();
      await page.goForward();
      await page.getByRole('heading', { name: 'Machines', exact: true }).waitFor();
      await page.evaluate(() => {
        window.holdHost = true;
        const link = document.createElement('a');
        link.href = '/account';
        document.querySelector('#page').append(link);
        link.click();
      });
      await page.waitForFunction(() => typeof window.releaseHost === 'function');
      await navigate('sessions');
      await page.locator('#page [data-session-row="list"]').waitFor();
      await page.evaluate(() => window.releaseHost());
      await page.waitForTimeout(50);
      assert.equal(await page.getByText('Stale account', { exact: true }).count(), 0);
      assert.deepEqual(
        await page.evaluate(() => ({
          sameApp: sharedApp === document.querySelector('.app'),
          sameSidebar: sharedSidebar === document.querySelector('#sidebar'),
          aborted: hostAborted,
        })),
        { sameApp: true, sameSidebar: true, aborted: true },
      );
      await page.evaluate(() => {
        window.holdHost = false;
        const fetch0 = window.fetch;
        window.fetch = (url, options) =>
          String(url).startsWith('/api/session-capabilities')
            ? new Promise((resolve) => {
                window.releaseCapability = () =>
                  resolve(
                    new Response(
                      JSON.stringify({
                        api: 1,
                        read_contract: 'catalog-v1',
                        source_key: 'other',
                        selected_transcript: true,
                        pagination: true,
                        filters: [],
                        order: 'last_desc_key_asc',
                        full_text_search: false,
                      }),
                      { headers: { 'Content-Type': 'application/json' } },
                    ),
                  );
              })
            : fetch0(url, options);
        const link = document.createElement('a');
        link.href = '/sessions?machine=other';
        document.querySelector('#page').append(link);
        link.click();
      });
      await page.waitForFunction(() => typeof window.releaseCapability === 'function');
      await navigate('machines');
      await page.getByRole('heading', { name: 'Machines', exact: true }).waitFor();
      const mountedBefore = await page.evaluate(() => hostMounted);
      await page.evaluate(() => window.releaseCapability());
      await page.waitForTimeout(50);
      assert.equal(
        await page.evaluate(() => hostMounted),
        mountedBefore,
        'stale capability reopened the host page',
      );
      await page.getByRole('heading', { name: 'Machines', exact: true }).waitFor();
      await page.evaluate(() => {
        window.failHost = true;
        const link = document.createElement('a');
        link.href = '/account';
        document.querySelector('#page').append(link);
        link.click();
      });
      await page.getByText('This page is temporarily unavailable.', { exact: false }).waitFor();
      assert.equal(await page.locator('#topbar .t').textContent(), 'Account');
      await page.evaluate(() => {
        window.failHost = false;
      });
      await page.getByRole('button', { name: 'Retry', exact: true }).click();
      await page.getByRole('heading', { name: 'Account', exact: true }).waitFor();
      await page.evaluate(() => app.destroy());
      assert.equal(
        await page.evaluate(() => hostDestroyed),
        (await page.evaluate(() => hostMounted)) + 1,
        'unmounted stale delivery is disposed too',
      );
      await page.evaluate(() => {
        window.workspaceReads = [];
        window.fetch = async (url) => {
          window.workspaceReads.push(String(url));
          throw Error('workspaceless read');
        };
        const element = document.createElement('section');
        element.textContent = 'No workspace';
        window.app = CatalogViewer.mountViewerApplication({
          machinesPath: '/machines',
          workspaceMetadata: false,
          pages: {
            match: () => ({ title: 'Account', nav: 'account' }),
            initial: {
              element,
              title: 'Account',
              nav: 'account',
              destroy() {
                element.remove();
              },
            },
            load: async () => null,
          },
        });
      });
      await page.getByText('No workspace', { exact: true }).waitFor();
      await page.waitForTimeout(25);
      assert.deepEqual(
        await page.evaluate(() => workspaceReads),
        [],
        'workspaceless host read workspace metadata',
      );
      await page.evaluate(() => app.destroy());
      assert.deepEqual(errors, []);
    } finally {
      await browser.close();
    }
  });

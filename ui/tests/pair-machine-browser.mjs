import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import { join } from 'node:path';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const css = await fs.readFile(
  new URL('../../crates/semon-sessions/src/viewer.css', import.meta.url),
  'utf8',
);
const shellCss = await fs.readFile(
  new URL('../../crates/semon-sessions/src/shell.css', import.meta.url),
  'utf8',
);
const { outputFiles } = await build({
  stdin: {
    contents:
      "export {openPairMachine} from './src/app/pairMachine'; export {renderCatalogSources} from './src/lib/catalogSources'; export {renderMachinesScreen,releaseScreen} from './src/lib/screens';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'PairFixture',
  platform: 'browser',
});
for (const width of [390, 820, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`receiver storage full ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const context = await browser.newContext({
          viewport: { width, height: 860 },
          colorScheme,
        });
        const page = await context.newPage();
        const errors = [];
        page.on('pageerror', (error) => errors.push(error.message));
        await page.setContent(
          `<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><style>${css}\n${shellCss}</style><main id="page" class="page"></main>`,
        );
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.machineClicks = [];
          PairFixture.renderMachinesScreen(
            document.querySelector('#page'),
            {
              up: 1,
              working: 0,
              admin: null,
              rows: [
                {
                  id: 'machine-12345678abcdef01',
                  name: 'Synthetic laptop',
                  state: 'idle',
                  stateLabel: 'Idle',
                  status: 'idle',
                  detail: '0 working · 2 sessions',
                },
              ],
              storageRefusals: [
                { machine: 'Synthetic laptop', reason: 'receiver_storage_full', time: '12:30' },
                {
                  machine: 'machine-12345678abcdef02',
                  reason: 'machine_storage_full',
                  time: '12:31',
                },
              ],
            },
            {
              machine(id) {
                window.machineClicks.push(id);
              },
              admin() {},
              committed() {},
            },
          );
        });
        const warning = page.getByRole('status', { name: 'Push storage refusals' });
        await warning
          .getByText('Receiver storage is full. Free space on the receiver to resume pushes.', {
            exact: true,
          })
          .waitFor();
        assert.match(
          await warning.innerText(),
          /Synthetic laptop — pushes refused: receiver storage full · 12:30/,
        );
        assert.match(
          await warning.innerText(),
          /machine-12345678abcdef02 — pushes refused: machine storage limit · 12:31/,
        );
        assert.equal(await warning.locator('li').count(), 2);
        assert.equal(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
          true,
        );
        if (process.env.SEMON_CATALOG_OUT) {
          await fs.mkdir(process.env.SEMON_CATALOG_OUT, { recursive: true });
          await page.screenshot({
            path: join(
              process.env.SEMON_CATALOG_OUT,
              `receiver-storage-full-${width}-${colorScheme}.png`,
            ),
            fullPage: true,
          });
        }
        await page.getByRole('button', { name: /Synthetic laptop/ }).click();
        assert.deepEqual(await page.evaluate(() => window.machineClicks), [
          'machine-12345678abcdef01',
        ]);
        await page.evaluate(() => {
          PairFixture.renderMachinesScreen(
            document.querySelector('#page'),
            { up: 0, working: 0, rows: [], admin: null, storageRefusals: [] },
            { machine() {}, admin() {}, committed() {} },
          );
        });
        assert.equal(await page.getByRole('status').count(), 0, 'warning clears on recovery');
        await page.evaluate(() => PairFixture.releaseScreen(document.querySelector('#page')));
        assert.equal(await page.locator('#page').innerText(), '');
        assert.deepEqual(errors, []);
        await context.close();
      } finally {
        await browser.close();
      }
    });
for (const width of [390, 820, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`pairing panel and empty receiver ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const context = await browser.newContext({
          viewport: { width, height: 860 },
          colorScheme,
          permissions: ['clipboard-read', 'clipboard-write'],
        });
        const page = await context.newPage();
        let issues = 0;
        const errors = [];
        page.on('pageerror', (error) => errors.push(error.message));
        await page.route('http://localhost:18273/**', (route) => {
          if (new URL(route.request().url()).pathname === '/api/viewer/pair') {
            assert.equal(route.request().method(), 'POST');
            issues++;
            return route.fulfill({
              json: {
                code: issues === 1 ? '01234-5678X' : 'ABCDE-FGHJK',
                expires: Math.floor(Date.now() / 1000) + 600,
              },
            });
          }
          return route.fulfill({
            contentType: 'text/html',
            body: `<!doctype html><meta name="viewport" content="width=device-width,initial-scale=1"><style>${css}\n${shellCss}</style><main id="page" class="page"></main>`,
          });
        });
        await page.goto('http://localhost:18273/');
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.pairOwner = null;
          PairFixture.renderCatalogSources(
            document.querySelector('#page'),
            {
              items: [],
              updating: false,
              note: '',
              more: false,
              machinesHref: '/machines',
              pair() {
                window.pairOwner = PairFixture.openPairMachine(
                  { opened() {}, closed() {} },
                  document.querySelector('button'),
                );
              },
            },
            { select() {}, more() {}, retry() {} },
          );
        });
        const capture = async (state) => {
          if (!process.env.SEMON_CATALOG_OUT) return;
          await fs.mkdir(process.env.SEMON_CATALOG_OUT, { recursive: true });
          await page.screenshot({
            path: join(process.env.SEMON_CATALOG_OUT, `pair-${state}-${width}-${colorScheme}.png`),
          });
        };
        await page.getByRole('heading', { name: 'Pair your first machine', exact: true }).waitFor();
        await page
          .getByText(
            'Run one command on each machine whose sessions you want here. Sync starts automatically.',
            { exact: true },
          )
          .waitFor();
        assert.equal(await page.getByRole('link', { name: 'Manage machines' }).count(), 0);
        assert.equal(
          await page.getByRole('button', { name: 'Pair a machine', exact: true }).count(),
          1,
        );
        await capture('empty');
        await page.getByRole('button', { name: 'Pair a machine', exact: true }).click();
        await page
          .getByText('semon pair http://localhost:18273 01234-5678X', { exact: true })
          .waitFor();
        await capture('panel');
        assert.equal(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
          true,
        );
        await page.getByRole('button', { name: 'Copy command', exact: true }).click();
        assert.equal(
          await page.evaluate(() => navigator.clipboard.readText()),
          'semon pair http://localhost:18273 01234-5678X',
        );
        await page.getByRole('button', { name: 'Get a new code', exact: true }).click();
        await page
          .getByText('semon pair http://localhost:18273 ABCDE-FGHJK', { exact: true })
          .waitFor();
        await page.keyboard.press('Escape');
        assert.equal(await page.getByRole('dialog').count(), 0);
        await page.getByRole('button', { name: 'Pair a machine', exact: true }).click();
        await page.evaluate(() => window.pairOwner.destroy());
        assert.equal(await page.getByRole('dialog').count(), 0);
        assert.deepEqual(errors, []);
        await context.close();
      } finally {
        await browser.close();
      }
    });

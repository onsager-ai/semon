import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';

const css = fs.readFileSync(
  new URL('../../crates/semon-sessions/src/viewer.css', import.meta.url),
  'utf8',
);
const { outputFiles } = await build({
  stdin: {
    contents:
      "export {renderSessionScreen} from './src/lib/transcript'; export {releaseScreen} from './src/lib/screens';",
    resolveDir: new URL('../', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'ReadingNotice',
  platform: 'browser',
});

for (const width of [390, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`saved reading notice remains passive and preserves responsive spacing at ${width}px ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({
            viewport: { width, height: 844 },
            isMobile: width === 390,
            hasTouch: width === 390,
            colorScheme,
          }),
          requests = [],
          errors = [];
        page.on('pageerror', (error) => errors.push(error.message));
        await page.route('http://reading.test/**', (route) => {
          const pathname = new URL(route.request().url()).pathname;
          requests.push(pathname);
          return route.fulfill({
            contentType: 'text/html',
            status: pathname === '/' ? 200 : 404,
            body:
              pathname === '/'
                ? `<!doctype html><meta name="viewport" content="width=device-width, initial-scale=1"><style>${css}</style><div class="app"><aside class="sidebar"></aside><main id="main" class="main"><header id="topbar" class="topbar"></header><div id="page" class="page"><div id="reader" class="catalog-session"></div></div></main></div>`
                : '',
          });
        });
        await page.goto('http://reading.test/');
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.reader = document.querySelector('#reader');
          window.actions = [];
          const blocks = [
            {
              kind: 'loose',
              key: 'saved-entry',
              entries: [
                {
                  kind: 'message',
                  key: 'saved-entry',
                  entryKey: 'saved-entry',
                  flavor: 'assistant',
                  text: 'The saved response remains readable.',
                },
              ],
            },
          ];
          ReadingNotice.renderSessionScreen(
            reader,
            {
              id: 'saved-session',
              name: 'Saved session',
              observation: 'Cached history',
              blocks,
              order: [],
              footer: { state: 'done', stateLabel: 'Done', text: 'Done', items: [] },
            },
            {
              committed() {},
              ...Object.fromEntries(
                [
                  'trace',
                  'session',
                  'machine',
                  'sender',
                  'toolAll',
                  'script',
                  'image',
                  'background',
                  'pager',
                  'jump',
                ].map((key) => [key, () => actions.push(key)]),
              ),
            },
          );
          blocks.map = () => {
            throw Error('Passive disclosure walked the saved transcript');
          };
        });
        const button = page.getByRole('button', {
            name: 'Native session controls are unavailable.',
            exact: true,
          }),
          explanation = page.getByText(
            'Saved session history remains readable. Session controls are unavailable here.',
            { exact: true },
          ),
          transcript = await page.locator('[data-entry-key="saved-entry"]').elementHandle();
        await button.waitFor();
        assert.equal(await button.getAttribute('aria-expanded'), 'false');
        assert.equal(await explanation.count(), 0);
        const geometry = await button.evaluate((node) => ({
          button: node.getBoundingClientRect().height,
          dock: node.closest('.session-dock').getBoundingClientRect().height,
          pageGap: getComputedStyle(document.querySelector('#page')).rowGap,
          readerGap: getComputedStyle(document.querySelector('#reader')).rowGap,
        }));
        assert.equal(geometry.pageGap, width === 390 ? '16px' : '24px');
        assert.equal(geometry.readerGap, geometry.pageGap);
        assert.equal(geometry.button, width === 390 ? 44 : 36);
        assert.equal(geometry.dock, geometry.button + 8);
        const noActivePaths = async () => {
          assert.equal(
            await page.locator('textarea, input, form, .session-composer-panel').count(),
            0,
          );
          for (const name of ['Open Composer', 'Send', 'Stop response', 'Reconnect', 'Allow once'])
            assert.equal(await page.getByRole('button', { name, exact: true }).count(), 0);
          assert.deepEqual(await page.evaluate(() => actions), []);
          assert.deepEqual(
            requests.filter((pathname) => pathname.startsWith('/api/')),
            [],
          );
        };
        await noActivePaths();
        await button.focus();
        await page.keyboard.press('Enter');
        await explanation.waitFor();
        assert.equal(await button.getAttribute('aria-expanded'), 'true');
        assert.equal(await button.evaluate((node) => node === document.activeElement), true);
        assert.equal(
          await page.evaluate(
            (node) => node === document.querySelector('[data-entry-key="saved-entry"]'),
            transcript,
          ),
          true,
        );
        await noActivePaths();
        await page.keyboard.press('Space');
        await explanation.waitFor({ state: 'detached' });
        assert.equal(await button.getAttribute('aria-expanded'), 'false');
        if (width === 390) {
          await button.tap();
          await explanation.waitFor();
          await button.tap();
          await explanation.waitFor({ state: 'detached' });
        }
        await page.evaluate(() => ReadingNotice.releaseScreen(reader));
        await button.evaluate((node) => node.click());
        assert.equal(await button.getAttribute('aria-expanded'), 'false');
        await noActivePaths();
        assert.deepEqual(errors, []);
      } finally {
        await browser.close();
      }
    });

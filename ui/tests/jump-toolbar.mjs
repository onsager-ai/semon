import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { readFile, mkdir } from 'node:fs/promises';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const { outputFiles } = await build({
  stdin: {
    contents: `export {createViewerBar} from './src/lib/viewer-bar'; export {renderSessionScreen, updateSessionJump} from './src/lib/transcript';`,
    resolveDir: new URL('../', import.meta.url).pathname,
    loader: 'tsx',
  },
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'UI',
  platform: 'browser',
  tsconfig: new URL('../tsconfig.json', import.meta.url).pathname,
});
const out = new URL('../../tests/ui/out-jump-toolbar/', import.meta.url);
await mkdir(out, { recursive: true });
const css = await readFile(
  new URL('../../crates/semon-sessions/src/viewer.css', import.meta.url),
  'utf8',
);
for (const width of [320, 390, 1280])
  for (const colorScheme of ['light', 'dark'])
    test(`reading dock jump preserves content and focus ${width} ${colorScheme}`, async () => {
      const browser = await chromium.launch();
      try {
        const page = await browser.newPage({ viewport: { width, height: 844 }, colorScheme });
        await page.route('http://jump.test/**', (r) =>
          r.fulfill({
            contentType: 'text/html',
            body: `<!doctype html><style>${css}</style><body><div id="topbar" class="topbar session-bar"></div><div id="page" class="page"></div><textarea id="draft">retained draft</textarea>`,
          }),
        );
        await page.goto('http://jump.test/');
        await page.addScriptTag({ content: outputFiles[0].text });
        await page.evaluate(() => {
          window.jumps = 0;
          window.bar = UI.createViewerBar();
          window.view = {
            mode: 'normal',
            name: 'A long session title that needs clipping on narrow screens',
            session: 's',
            showState: false,
            ancestors: [],
            labels: [],
            trace: false,
            analytics: false,
            days: 7,
            query: '',
            count: '',
            filter: 'all',
            failed: 0,
            signals: 0,
            errorMode: 'errors',
            errorText: '',
            errorDisabled: false,
            icons: { search: 'M1 1', more: 'M2 2', back: '', x: '', up: '', dn: '' },
          };
          window.host = {
            ancestor() {},
            crumb() {},
            find() {},
            closeFind() {},
            query() {},
            filter() {},
            menu() {},
            errors() {},
            closeErrors() {},
            step() {},
            range() {},
          };
          const parts = bar.update(view, host);
          topbar.append(parts.titleSlot, parts.actions, parts.mode);
          window.snapshot = {
            id: 's',
            name: view.name,
            blocks: [],
            order: [],
            empty: 'Retained conversation content',
          };
          UI.renderSessionScreen(document.querySelector('#page'), snapshot, {
            committed() {},
            jump() {
              jumps++;
            },
          });
          UI.updateSessionJump(document.querySelector('#page'), true, 3, false, bar.jumpTarget);
          window.content = document.querySelector('.turns');
          window.button = document.querySelector('#jump-bottom');
        });
        const button = page.locator('#jump-bottom');
        await button.focus();
        await page.evaluate(() => {
          bar.update({ ...view, name: 'Updated title' }, host);
          UI.updateSessionJump(document.querySelector('#page'), true, 4, false, bar.jumpTarget);
        });
        assert.equal(await page.evaluate(() => document.activeElement === button), true);
        assert.equal(await button.getAttribute('aria-label'), 'Jump to bottom; 4 new entries');
        const geometry = await page.evaluate(() => {
          const b = button.getBoundingClientRect(),
            t = document.querySelector('.session-dock').getBoundingClientRect(),
            c = content.getBoundingClientRect(),
            f = document.querySelector('#find-btn').getBoundingClientRect(),
            m = document.querySelector('#more-btn').getBoundingClientRect();
          return {
            inside: b.top >= t.top && b.bottom <= t.bottom && b.left >= 0 && b.right <= innerWidth,
            clear: b.top >= c.bottom,
            targets:
              b.width >= (innerWidth <= 760 ? 44 : 40) && b.height >= (innerWidth <= 760 ? 44 : 40),
            actions: b.right <= f.left && f.right <= m.left,
            overflow: document.documentElement.scrollWidth > innerWidth,
          };
        });
        assert.deepEqual(geometry, {
          inside: true,
          clear: true,
          targets: true,
          actions: true,
          overflow: false,
        });
        await page.screenshot({ path: new URL(`${width}-${colorScheme}.png`, out).pathname });
        await page.keyboard.press('Enter');
        assert.equal(await page.evaluate(() => jumps), 1);
        await page.locator('#draft').focus();
        await page.evaluate(() => {
          UI.renderSessionScreen(
            document.querySelector('#page'),
            { ...snapshot, name: 'Fresh name' },
            {
              committed() {},
              jump() {
                jumps++;
              },
            },
          );
          UI.updateSessionJump(document.querySelector('#page'), false, 0, false, bar.jumpTarget);
        });
        assert.equal(await page.locator('#draft').inputValue(), 'retained draft');
        assert.equal(await page.evaluate(() => document.activeElement.id), 'draft');
        assert.equal(await page.evaluate(() => content === document.querySelector('.turns')), true);
        assert.equal(await button.isVisible(), false);
        await page.evaluate(() => {
          UI.updateSessionJump(document.querySelector('#page'), true, 0, false, bar.jumpTarget);
          bar.update({ ...view, mode: 'find' }, host);
        });
        assert.equal(await button.isVisible(), true);
        assert.equal(
          await page.evaluate(() => button === document.querySelector('#jump-bottom')),
          true,
        );
      } finally {
        await browser.close();
      }
    });

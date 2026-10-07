import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { readFile } from 'node:fs/promises';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
import { PNG } from '../../tests/ui/node_modules/pngjs/lib/png.js';

const { outputFiles } = await build({
  entryPoints: [new URL('../src/lib/richtext.tsx', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'RichText',
  platform: 'browser',
});
const css = await readFile(
  new URL('../../crates/semon-sessions/src/viewer.css', import.meta.url),
  'utf8',
);
for (const dark of [false, true])
  test(`long code hints only the edges with more content (${dark ? 'dark' : 'light'})`, async () => {
    const browser = await chromium.launch();
    try {
      const page = await browser.newPage({
        viewport: { width: 390, height: 844 },
        colorScheme: dark ? 'dark' : 'light',
      });
      await page.route('http://code.test/**', (route) =>
        route.fulfill({
          contentType: 'text/html',
          body:
            '<!doctype html><style>' +
            css +
            '</style><body><div id="mount" style="width:358px;margin:16px"></div>',
        }),
      );
      await page.goto('http://code.test/');
      await page.addScriptTag({ content: outputFiles[0].text });
      await page.evaluate(() => {
        mount.append(RichText.createMarkdown('```text\n' + 'long_code_'.repeat(50) + '\n```'));
        mount.append(RichText.createMarkdown('```text\nshort\n```'));
      });
      const code = page.locator('.codeblock').first();
      const hints = async (box) => {
        const png = PNG.sync.read(await box.screenshot());
        const color = (x) => {
          const i = ((png.height - 5) * png.width + x) * 4;
          return [...png.data.subarray(i, i + 3)];
        };
        const center = color(Math.floor(png.width / 2));
        const difference = (edge) =>
          Math.max(...color(edge).map((v, i) => Math.abs(v - center[i])));
        return { left: difference(8), right: difference(png.width - 9) };
      };
      assert.deepEqual(await hints(page.locator('.codeblock').last()), { left: 0, right: 0 });
      const start = await hints(code);
      assert.equal(start.left, 0);
      assert.ok(start.right >= 5, JSON.stringify(start));
      await code.focus();
      await page.keyboard.press('ArrowRight');
      await page.waitForFunction(() => document.querySelector('.codeblock').scrollLeft > 0);
      await code.evaluate((box) => (box.scrollLeft = box.scrollWidth));
      await page.waitForFunction(() => {
        const box = document.querySelector('.codeblock');
        return box.scrollWidth - box.clientWidth - box.scrollLeft <= 1;
      });
      const end = await hints(code);
      assert.ok(end.left >= 5, JSON.stringify(end));
      assert.equal(end.right, 0);
      assert.equal(
        await page.evaluate(() => document.activeElement === document.querySelector('.codeblock')),
        true,
      );
      assert.equal(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
        true,
      );
      await page.evaluate(() => {
        document.querySelector('.codeblock pre').textContent = 'now short';
      });
      assert.deepEqual(await hints(code), { left: 0, right: 0 });
    } finally {
      await browser.close();
    }
  });

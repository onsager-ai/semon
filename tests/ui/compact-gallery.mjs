import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import { buildConsumer } from '../../ui/build.mjs';
import { launch } from './lib.mjs';
import { auditText } from './text-audit.mjs';
const root = path.resolve(import.meta.dirname, '../..');
const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'compact-gallery-'));
const output = path.join(temp, 'gallery.js');
await buildConsumer(path.join(root, 'ui/tests/fixtures/compact-gallery.ts'), output);
const out = path.resolve(process.env.SEMON_UI_OUT ?? 'out', 'compact-gallery');
fs.mkdirSync(out, { recursive: true });
const svg =
  '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h16M4 18h16M8 3v6M16 15v6" stroke="currentColor" fill="none"></path></svg>';
const html = `<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/viewer.css"><link rel="stylesheet" href="/shell.css"><link rel="stylesheet" href="/select.css"><script src="/gallery.js" defer></script></head><body><main class="page"><h1>Compact composer</h1><form class="sh-composer"><textarea aria-label="Task" placeholder="What would you like to work on?"></textarea><div class="sh-composer-toolbar"><details class="sh-picker"><summary class="sh-compact sh-value" aria-label="Model: a long catalog model label; effort: high" data-tip="Model: a long catalog model label; effort: high">${svg}Codex</summary><div class="sh-picker-body"><h2>Model and effort</h2><label class="field">Model<select name="model" aria-label="Model"><option value="default">A long catalog model label with additional account qualification detail</option><option value="second">Second qualified model</option></select></label></div></details><details class="sh-picker"><summary class="sh-compact sh-value" aria-label="Approval permissions" data-tip="Approval permissions">${svg}Managed</summary><div class="sh-picker-body"><h2>Approval permissions</h2><p>Read-only pending independent qualification.</p></div></details><details class="sh-picker"><summary class="sh-compact sh-value" aria-label="Runtime">${svg}E2B</summary><div class="sh-picker-body"><h2>Runtime</h2><p>Read-only runtime configuration.</p></div></details><button class="sh-compact sh-composer-send primary" disabled aria-label="Review launch">${svg}</button></div></form><p><button id="remount" class="btn">Remount</button></p></main></body></html>`;
const browser = await launch();
try {
  for (const width of [390, 820, 1280])
    for (const colorScheme of ['light', 'dark']) {
      const context = await browser.newContext({
        viewport: { width, height: 844 },
        colorScheme,
        hasTouch: width < 1000,
        reducedMotion: 'reduce',
      });
      const page = await context.newPage();
      await page.route('**/*', (route) => {
        const name = new URL(route.request().url()).pathname;
        const file =
          name === '/gallery.js'
            ? output
            : ['viewer.css', 'shell.css', 'select.css'].includes(name.slice(1))
              ? path.join(root, 'crates/semon-sessions/src', name.slice(1))
              : name.startsWith('/fonts/')
                ? path.join(
                    root,
                    'crates/semon-sessions/src/fonts',
                    name.slice(7).replace(/-(latin(?:-ext)?)\.woff2$/, '/$1.woff2'),
                  )
                : null;
        return route.fulfill({
          body: file ? fs.readFileSync(file) : html,
          contentType: name.endsWith('.js')
            ? 'text/javascript'
            : name.endsWith('.css')
              ? 'text/css'
              : name.endsWith('.woff2')
                ? 'font/woff2'
                : 'text/html',
        });
      });
      await page.goto('http://compact.test');
      const prompt = page.getByLabel('Task');
      assert.equal(await prompt.evaluate((n) => n.getBoundingClientRect().height), 96);
      await prompt.fill(Array.from({ length: 30 }, (_, i) => `Prompt line ${i}`).join('\n'));
      assert.equal(await prompt.evaluate((n) => n.getBoundingClientRect().height), 280);
      const buttons = page.locator('.sh-composer-toolbar > button, .sh-picker > summary');
      const boxes = await buttons.evaluateAll((nodes) =>
        nodes.map((n) => {
          const b = n.getBoundingClientRect();
          return { x: b.x, end: b.right, y: b.y + b.height / 2, height: b.height };
        }),
      );
      assert(boxes.every((b) => Math.abs(b.y - boxes[0].y) < 1));
      if (width < 1000) assert(boxes.every((b) => b.height >= 44));
      for (let i = 1; i < boxes.length; i++) assert(boxes[i].x >= boxes[i - 1].end);
      const audit = await auditText(page);
      assert.equal(audit.smallCount, 0);
      assert.equal(audit.lowCount, 0);
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      await page.screenshot({
        path: path.join(out, `composer-${width}-${colorScheme}.png`),
        fullPage: true,
      });
      const trigger = page.locator('.sh-picker summary').first();
      await trigger.focus();
      await page.keyboard.press('Enter');
      await page.screenshot({
        path: path.join(out, `picker-${width}-${colorScheme}.png`),
        fullPage: true,
      });
      await page.getByRole('combobox').click();
      const options = page.getByRole('option');
      assert(await options.first().evaluate((n) => getComputedStyle(n).fontSize === '13px'));
      assert(await options.first().evaluate((n) => n.getBoundingClientRect().height >= 32));
      await page.keyboard.press('End');
      assert.equal(
        await options
          .last()
          .getAttribute('class')
          .then((c) => c.includes('sh-active')),
        true,
      );
      assert.equal(await options.first().getAttribute('aria-selected'), 'true');
      await page.getByRole('option', { name: 'Second qualified model' }).click();
      assert.equal(await page.locator('select[name=model]').inputValue(), 'second');
      assert.equal(
        await page.locator('form').evaluate((n) => new FormData(n).get('model')),
        'second',
      );
      if (width === 390) {
        await page.getByRole('combobox').click();
        await page.goBack();
        await page.locator('dialog.sh-select-sheet').waitFor({ state: 'detached' });
        assert(await page.locator('dialog.sh-composer-panel').isVisible());
      }
      assert.equal(
        await page
          .locator('.sh-select-trigger')
          .first()
          .evaluate((n) => n.getBoundingClientRect().height),
        width < 1000 ? 40 : 36,
      );
      await page.keyboard.press('Escape');
      await page.locator('.sh-picker[open]').waitFor({ state: 'detached' });
      assert.equal(await trigger.evaluate((n) => document.activeElement === n), true);
      assert.equal(await page.locator('.sh-picker[open]').count(), 0);
      await page.getByRole('button', { name: 'Remount' }).click();
      await trigger.click();
      await page.keyboard.press('Escape');
      await page.locator('.sh-picker[open]').waitFor({ state: 'detached' });
      assert.equal(await page.locator('.sh-picker[open]').count(), 0);
      // Real consumer labels must fit without spending the send action's inset.
      for (const agent of ['Codex', 'Copilot']) {
        await trigger.evaluate((node, value) => {
          node.lastChild.textContent = value;
        }, agent);
        const geometry = await page.locator('.sh-composer-toolbar').evaluate((node) => {
          const frame = node.parentElement.getBoundingClientRect();
          const controls = [
            ...node.querySelectorAll(':scope > button, :scope > details > summary'),
          ];
          const first = controls[0],
            last = controls.at(-1);
          return {
            left:
              first.getBoundingClientRect().left -
              frame.left +
              parseFloat(getComputedStyle(first, '::before').left),
            right:
              frame.right -
              last.getBoundingClientRect().right +
              parseFloat(getComputedStyle(last, '::before').right),
          };
        });
        assert(Math.abs(geometry.left - 17) < 1, `${agent}: left inset ${geometry.left}`);
        assert(Math.abs(geometry.right - 17) < 1, `${agent}: right inset ${geometry.right}`);
      }
      await context.close();
    }
  console.log(
    'Compact gallery: 6 light/dark/width cases; growth, targets, centers, three labeled controls/insets, text, keyboard and remount passed',
  );
} finally {
  await browser.close();
  fs.rmSync(temp, { recursive: true, force: true });
}

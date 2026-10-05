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
const html = `<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/viewer.css"><link rel="stylesheet" href="/shell.css"><script src="/gallery.js" defer></script></head><body><main class="page"><h1>Compact composer</h1><form class="sh-composer"><textarea aria-label="Task" placeholder="What would you like to work on?"></textarea><div class="sh-composer-toolbar"><details class="sh-picker"><summary class="sh-compact" aria-label="Model: a long catalog model label; effort: high" data-tip="Model: a long catalog model label; effort: high">${svg}</summary><div class="sh-picker-body"><h2>Model and effort</h2><label class="field">Model<select><option>A long catalog model label with additional account qualification detail</option></select></label></div></details><details class="sh-picker"><summary class="sh-compact sh-value" aria-label="Approval permissions" data-tip="Approval permissions">${svg}Managed</summary><div class="sh-picker-body"><h2>Approval permissions</h2><p>Read-only pending independent qualification.</p></div></details><button class="sh-compact sh-composer-send primary" disabled aria-label="Review launch">${svg}</button></div></form><p><button id="remount" class="btn">Remount</button></p></main></body></html>`;
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
            : ['viewer.css', 'shell.css'].includes(name.slice(1))
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
      await page.keyboard.press('Escape');
      assert.equal(await trigger.evaluate((n) => document.activeElement === n), true);
      assert.equal(await page.locator('.sh-picker[open]').count(), 0);
      await page.getByRole('button', { name: 'Remount' }).click();
      await trigger.click();
      await page.keyboard.press('Escape');
      assert.equal(await page.locator('.sh-picker[open]').count(), 0);
      await context.close();
    }
  console.log(
    'Compact gallery: 6 light/dark/width cases; growth, targets, centers, text, keyboard and remount passed',
  );
} finally {
  await browser.close();
  fs.rmSync(temp, { recursive: true, force: true });
}

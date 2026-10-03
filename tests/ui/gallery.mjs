import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { buildConsumer } from '../../ui/build.mjs';
import { launch } from './lib.mjs';
import { auditText } from './text-audit.mjs';
import { createVisualContract } from './visual-contract.mjs';

const dir = path.dirname(fileURLToPath(import.meta.url)), root = path.resolve(dir, '../..');
const args = process.argv.slice(2);
if (args.length && (args.length !== 1 || args[0] !== '--update-baselines')) throw new Error('Usage: node tests/ui/gallery.mjs [--update-baselines]');
const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'semon-gallery-'));
const output = path.join(temp, 'gallery.js');
await buildConsumer(path.join(root, 'ui/tests/fixtures/design-gallery.tsx'), output);
const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'gallery-visual.json'), 'utf8'));
const out = path.resolve(process.env.SEMON_UI_OUT ?? path.join(dir, 'out'), 'gallery');
const visuals = createVisualContract({ manifest, baselineDir: path.join(dir, 'baselines/gallery'), outDir: out, update: args.includes('--update-baselines') });
const assets = new Map([
  ['/gallery.js', [output, 'text/javascript']], ['/viewer.css', [path.join(root, 'crates/semon-sessions/src/viewer.css'), 'text/css']],
  ['/select.css', [path.join(root, 'crates/semon-sessions/src/select.css'), 'text/css']], ['/shell.css', [path.join(root, 'crates/semon-sessions/src/shell.css'), 'text/css']],
  ['/gallery.css', [path.join(dir, 'gallery.css'), 'text/css']], ['/mark.svg', [path.join(root, 'crates/semon-sessions/src/mark.svg'), 'image/svg+xml']],
]);
for (const family of ['instrument-sans', 'jetbrains-mono']) for (const range of ['latin', 'latin-ext']) assets.set(`/fonts/${family}-${range}.woff2`, [path.join(root, `crates/semon-sessions/src/fonts/${family}/${range}.woff2`), 'font/woff2']);
const html = '<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><link rel="stylesheet" href="/viewer.css"><link rel="stylesheet" href="/select.css"><link rel="stylesheet" href="/shell.css"><link rel="stylesheet" href="/gallery.css"><script defer src="/gallery.js"></script></head><body><div class="app"></div></body></html>';
const browser = await launch();
const report = [];
try {
  for (const width of manifest.widths) for (const scheme of manifest.schemes) {
    const touch = width < 1000;
    const context = await browser.newContext({ viewport: { width, height: 844 }, colorScheme: scheme, deviceScaleFactor: width === 390 ? 2 : 1, isMobile: width === 390, hasTouch: touch, timezoneId: 'UTC', locale: 'en-US' });
    const page = await context.newPage(), errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/*', route => {
      const url = new URL(route.request().url());
      if (url.origin !== 'http://gallery.test') return route.abort();
      const asset = assets.get(url.pathname);
      return route.fulfill({ contentType: asset?.[1] ?? 'text/html', body: asset ? fs.readFileSync(asset[0]) : html, headers: { 'Content-Security-Policy': "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'" } });
    });
    await page.goto('http://gallery.test/');
    await page.locator('.sh-select-trigger').waitFor();
    const audit = async state => {
      const text = await auditText(page);
      assert.ok(text.texts > 0, 'empty audit'); assert.equal(text.smallCount, 0, JSON.stringify(text.small)); assert.equal(text.lowCount, 0, JSON.stringify(text.low));
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'sideways scroll');
      if (touch) {
        const short = await page.locator('.btn, .sh-select-trigger, .nav-item, dialog[open] button').evaluateAll(nodes => nodes.filter(node => node.getClientRects().length && getComputedStyle(node).visibility !== 'hidden' && node.getBoundingClientRect().height < 44).map(node => node.className));
        assert.deepEqual(short, [], 'touch target below 44px');
      }
      report.push({ width, scheme, state, text });
      await visuals.capture(page, `${state}-${width}-${scheme}`);
    };
    await audit('primitives');
    // Keyboard opens and selects through the real shared Select controller.
    await page.locator('.sh-select-trigger').focus(); await page.keyboard.press('ArrowDown');
    await page.locator('[role="option"]').first().waitFor(); await audit('select');
    await page.keyboard.press('ArrowDown'); await page.keyboard.press('Enter');
    await page.waitForFunction(() => document.querySelector('.sh-select-trigger')?.getAttribute('aria-expanded') === 'false');
    if (width === 390) { await page.locator('#lead-btn').click(); await page.locator('#account-drawer .account-trigger').click(); }
    else await page.locator('#topbar .account-trigger').click();
    await page.locator('.account-popover').waitFor(); await audit('account');
    await page.keyboard.press('Escape');
    if (width === 390) await page.keyboard.press('Escape');
    for (const state of ['sheet', 'panel']) {
      await page.locator(`#${state}-trigger`).click();
      await page.locator('dialog[open]').waitFor(); await audit(state);
      await page.keyboard.press('Escape'); await page.waitForFunction(() => !document.querySelector('dialog[open]'));
      assert.equal(await page.evaluate(() => document.activeElement?.id), `${state}-trigger`, 'focus return');
    }
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const moving = await page.locator('.dot.work, .sk-line').evaluateAll(nodes => nodes.filter(node => getComputedStyle(node).animationName !== 'none').map(node => node.className));
    assert.deepEqual(moving, [], 'reduced motion'); assert.deepEqual(errors, [], 'page errors');
    await context.close();
  }
  const failures = visuals.finish(); fs.writeFileSync(path.join(out, 'audit.json'), JSON.stringify(report, null, 2));
  assert.deepEqual(failures, [], 'gallery visual contract');
  console.log(`Component gallery: ${report.length} audited visual states; keyboard, touch and reduced motion passed`);
} finally { await browser.close(); fs.rmSync(temp, { recursive: true, force: true }); }

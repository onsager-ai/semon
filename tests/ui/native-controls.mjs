// Native consumers share compact density without losing forms, link semantics,
// readable primary actions or the 44px touch target.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { launch } from './lib.mjs';
import { auditText } from './text-audit.mjs';

const root = fileURLToPath(new URL('../..', import.meta.url));
const out = path.resolve(process.env.SEMON_UI_OUT ?? 'out', 'native-controls');
fs.mkdirSync(out, { recursive: true });
const assets = new Map(
  ['viewer', 'shell'].map((name) => [
    `/${name}.css`,
    [path.join(root, `crates/semon-sessions/src/${name}.css`), 'text/css'],
  ]),
);
for (const family of ['instrument-sans', 'jetbrains-mono'])
  for (const range of ['latin', 'latin-ext'])
    assets.set(`/fonts/${family}-${range}.woff2`, [
      path.join(root, `crates/semon-sessions/src/fonts/${family}/${range}.woff2`),
      'font/woff2',
    ]);
const html = `<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<link rel="stylesheet" href="/viewer.css"><link rel="stylesheet" href="/shell.css">
</head><body><main class="page">
<section><h2 class="sec-h">Native actions</h2><div class="btn-row sh-actions">
<button class="btn sh-compact primary" type="button">Continue</button>
<button class="btn sh-compact" type="button">Secondary</button>
<button class="btn sh-compact danger" type="button">Delete</button>
<button class="btn sh-compact sh-quiet" type="button">Cancel</button>
<button class="btn sh-compact" type="button" disabled>Unavailable</button>
</div></section>
<section><form method="get" action="/submitted"><label class="field">Workspace name
<input name="name" value="Synthetic workspace"></label>
<div class="btn-row sh-actions"><button class="btn sh-compact primary" type="submit">Save name</button></div>
</form></section>
<section><div class="btn-row sh-actions"><a class="btn sh-compact" href="/destination">
Return to your saved task with an intentionally long label that wraps on a phone</a></div></section>
<section><div class="btn-row sh-actions"><a class="sh-compact sh-value primary" href="/destination">New session</a></div></section>
<section class="notice"><a class="sh-compact sh-value primary" href="/destination">Recover</a></section>
<section class="sh-step"><a class="sh-compact sh-value" href="/destination">Inspect</a></section>
<section class="signin"><a class="sh-compact sh-value primary" href="/destination">Sign in</a></section>
</main></body></html>`;
const browser = await launch();
const report = [];
try {
  for (const width of [390, 820, 1280])
    for (const colorScheme of ['light', 'dark'])
      for (const javaScriptEnabled of [false, true]) {
        const touch = width < 1000;
        const context = await browser.newContext({
          viewport: { width, height: 844 },
          colorScheme,
          hasTouch: touch,
          javaScriptEnabled,
          reducedMotion: 'reduce',
        });
        const page = await context.newPage();
        await page.route('http://native.test/**', (route) => {
          const asset = assets.get(new URL(route.request().url()).pathname);
          return route.fulfill({
            contentType: asset?.[1] ?? 'text/html',
            body: asset ? fs.readFileSync(asset[0]) : html,
          });
        });
        await page.goto('http://native.test/');
        await page.evaluate(() => document.fonts.ready);
        const audit = await auditText(page);
        assert.equal(audit.smallCount, 0, JSON.stringify(audit.small));
        assert.equal(audit.lowCount, 0, JSON.stringify(audit.low));
        const controls = await page.locator('.sh-compact').evaluateAll((nodes) =>
          nodes.map((node) => {
            const style = getComputedStyle(node);
            const surface = getComputedStyle(node, '::before');
            const box = node.getBoundingClientRect();
            return {
              label: node.textContent.trim(),
              height: box.height,
              width: box.width,
              surfaceHeight: box.height - parseFloat(surface.top) - parseFloat(surface.bottom),
              fontSize: parseFloat(style.fontSize),
              underline: style.textDecorationLine.includes('underline'),
            };
          }),
        );
        for (const control of controls) {
          assert.ok(control.height >= (touch ? 44 : 32), JSON.stringify(control));
          assert.equal(control.underline, false, JSON.stringify(control));
          assert.equal(control.fontSize, 13, JSON.stringify(control));
          if (control.label === 'Continue') {
            assert.equal(control.surfaceHeight, 32);
            assert.ok(control.width < 150, 'compact action stretched across the page');
          }
        }
        assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
        await page.getByRole('button', { name: 'Continue', exact: true }).focus();
        await page.keyboard.press('Tab');
        await page.getByRole('button', { name: 'Secondary', exact: true }).evaluate((node) => {
          assertFocused(node);
          function assertFocused(element) {
            if (document.activeElement !== element) throw new Error('keyboard focus order');
          }
        });
        await page.screenshot({
          path: path.join(out, `${width}-${colorScheme}-${javaScriptEnabled}.png`),
        });
        await page.getByRole('button', { name: 'Save name', exact: true }).click();
        await page.waitForURL('**/submitted?name=Synthetic+workspace');
        await page.getByRole('link', { name: 'New session', exact: true }).click();
        await page.waitForURL('**/destination');
        report.push({ width, colorScheme, javaScriptEnabled, controls, audit });
        await context.close();
      }
} finally {
  await browser.close();
}
fs.writeFileSync(path.join(out, 'report.json'), JSON.stringify(report, null, 2));
console.log(`Native compact controls: ${report.length} viewport/theme/JavaScript cases passed`);

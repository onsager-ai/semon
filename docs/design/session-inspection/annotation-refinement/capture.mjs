import fs from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = fileURLToPath(new URL('../../../../', import.meta.url));
const { chromium } = await import(pathToFileURL(path.join(root, 'tests/ui/node_modules/playwright/index.mjs')));
const { auditText } = await import(pathToFileURL(path.join(root, 'tests/ui/text-audit.mjs')));
const preview = process.env.SEMON_PREVIEW_LOG || '/workspace/scratch/annotation-viewer-preview/server.log';
const address = (await fs.readFile(preview, 'utf8')).match(/http:\/\/127\.0\.0\.1:[0-9]+\/\?t=[a-f0-9]+/)?.[0];
assert(address, 'Start the isolated sanitized viewer fixture first');
const base = 'defe9d791a197902c5bebcba2e2b7fe87dd9a075';
const css = await fs.readFile(path.join(root, 'crates/semon-sessions/src/viewer.css'), 'utf8');
const js = await fs.readFile(path.join(root, 'crates/semon-sessions/src/viewer.generated.js'), 'utf8');
const baseline = name => execFileSync('git', ['show', base + ':crates/semon-sessions/src/' + name], { cwd: root, encoding: 'utf8' });
const browser = await chromium.launch({ args: ['--disable-gpu', '--font-render-hinting=none'] });
const cases = [];
try {
  for (const width of [390, 1280]) for (const colorScheme of ['light', 'dark']) {
    for (const before of [true, false]) {
      const context = await browser.newContext({ viewport: { width, height: 860 }, deviceScaleFactor: 2, hasTouch: width === 390, colorScheme });
      const page = await context.newPage();
      await page.clock.setFixedTime(Number((await fs.readFile(process.env.SEMON_FIXTURE_NOW || '/workspace/scratch/surface-fixture-now', 'utf8')).trim()));
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      if (before) {
        await page.route('**/viewer.css*', route => route.fulfill({ contentType: 'text/css', body: baseline('viewer.css') }));
        await page.route('**/viewer.js*', route => route.fulfill({ contentType: 'text/javascript', body: baseline('viewer.generated.js') }));
      }
      const url = new URL(address);
      url.searchParams.set('compat', '1');
      await page.goto(url.href);
      if (!before) {
        assert((await (await page.request.get(new URL('/viewer.css', address).href)).text()).includes(css));
        assert((await (await page.request.get(new URL('/viewer.js', address).href)).text()).includes(js));
      }
      await page.locator('#page [data-id="harbor"]').click();
      await page.locator('.child-card').first().waitFor();
      await page.evaluate(() => document.fonts.ready);
      const output = path.join(root, 'docs/design/session-inspection/annotation-refinement', before ? 'before' : 'after');
      await fs.mkdir(output, { recursive: true });
      await page.screenshot({ path: path.join(output, `reader-${width}-${colorScheme}.png`) });
      const heights = await page.locator('.child-card').evaluateAll(nodes => nodes.map(node => node.getBoundingClientRect().height));
      if (!before) {
        assert(heights.every(height => height <= (width === 390 ? 66 : 36)), JSON.stringify(heights));
        const child = page.locator('.child-card').first();
        const toggle = child.locator('.cc-toggle');
        assert.equal(await toggle.getAttribute('aria-expanded'), 'false');
        assert(!(await child.locator('.cc-brief').isVisible()));
        const route = await page.evaluate(() => history.state);
        await toggle.focus();
        await page.keyboard.press('Enter');
        assert(await child.locator('.cc-brief').isVisible());
        assert.equal(await child.locator('.cc-brief').evaluate(node => getComputedStyle(node).webkitLineClamp), 'none');
        const { scrollTop: beforeScroll, ...beforeRoute } = route;
        const { scrollTop: afterScroll, ...afterRoute } = await page.evaluate(() => history.state);
        assert.deepEqual(afterRoute, beforeRoute);
        await page.screenshot({ path: path.join(output, `expanded-${width}-${colorScheme}.png`) });
        await page.keyboard.press('Enter');
        assert.equal(await toggle.getAttribute('aria-expanded'), 'false');
        await child.locator('.cc-name').click();
        await page.waitForFunction(parent => history.state.id !== parent, route.id);
        await page.goBack();
        await page.waitForFunction(parent => history.state.id === parent, route.id);
        await page.locator('.child-card').first().waitFor();
        const audit = await auditText(page);
        assert.equal(audit.smallCount, 0);
        assert.equal(audit.lowCount, 0);
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
        assert.deepEqual(errors, []);
      }
      cases.push({ width, colorScheme, before, heights, compiledFrontend: !before, baselineOverlay: before, keyboardExpansion: !before, directNavigationAndBack: !before });
      await context.close();
    }
  }
} finally { await browser.close(); }
await fs.writeFile(path.join(root, 'docs/design/session-inspection/annotation-refinement/observations.json'), JSON.stringify({ baseline: base, sourceCssSha256: crypto.createHash('sha256').update(css).digest('hex'), bundleSha256: crypto.createHash('sha256').update(js).digest('hex'), cases }, null, 2) + '\n');
console.log('8 matched viewer cases; compact rows, keyboard expansion, direct navigation/Back, text and overflow passed');

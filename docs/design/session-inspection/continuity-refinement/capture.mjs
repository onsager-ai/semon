import fs from 'node:fs/promises';
import path from 'node:path';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { chromium } from '../../../../tests/ui/node_modules/playwright/index.mjs';
import { auditText } from '../../../../tests/ui/text-audit.mjs';
const root = new URL('../../../../', import.meta.url).pathname;
const out = new URL('.', import.meta.url).pathname;
const home = process.env.SEMON_REFINEMENT_FIXTURE;
assert(home && process.env.SEMON_BEFORE_BIN && process.env.SEMON_AFTER_BIN);
const fixture = spawnSync(process.execPath, [path.join(root, 'tests/ui/fixture.mjs'), home, '--extras'], { encoding: 'utf8' });
assert.equal(fixture.status, 0, fixture.stderr);
const now = fixture.stdout.trim();
const servers = [];
async function serve(binary, cache) {
  const server = spawn(binary, ['sessions', '--serve', '--listen', '127.0.0.1:0', '--claude-home', path.join(home, 'claude'), '--claude-json', path.join(home, '.claude.json'), '--codex-home', path.join(home, 'codex'), '--copilot-home', path.join(home, 'copilot'), '--proc-root', path.join(home, 'proc'), '--cache', path.join(home, cache)], { env: { ...process.env, SEMON_TEST_NOW: now }, stdio: ['ignore', 'pipe', 'pipe'] });
  servers.push(server);
  let log = '';
  for (const stream of [server.stdout, server.stderr]) stream.on('data', data => log += data);
  for (let attempt = 0; attempt < 200; attempt++) {
    const address = log.match(/http:\/\/127\.0\.0\.1:\d+\/\?t=[a-f0-9]+/)?.[0];
    if (address) return address;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw Error('Synthetic fixture did not start');
}
const browser = await chromium.launch({ args: ['--disable-gpu', '--font-render-hinting=none'] });
const observations = [];
try {
  const addresses = [await serve(process.env.SEMON_BEFORE_BIN, 'before-index.json'), await serve(process.env.SEMON_AFTER_BIN, 'after-index.json')];
  for (const width of [390, 820, 1280]) for (const colorScheme of ['light', 'dark']) for (const before of [true, false]) {
    const context = await browser.newContext({ viewport: { width, height: 860 }, colorScheme, hasTouch: width === 390 });
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.clock.setFixedTime(Number(now));
    const address = new URL(addresses[before ? 0 : 1]);
    address.searchParams.set('compat', '1');
    await page.goto(address.href);
    await page.locator('#page [data-id="harbor"]').click();
    const child = page.locator('.child-card').first();
    await child.waitFor();
    await page.evaluate(() => document.fonts.ready);
    await child.scrollIntoViewIfNeeded();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const directory = path.join(out, before ? 'before' : 'after');
    await fs.mkdir(directory, { recursive: true });
    await page.screenshot({ animations: 'disabled', path: path.join(directory, `compiled-reader-${width}-${colorScheme}.png`) });
    const toggle = child.locator('.cc-toggle');
    await toggle.focus();
    await toggle.press('Enter');
    assert.equal(await toggle.getAttribute('aria-expanded'), 'true');
    await child.scrollIntoViewIfNeeded();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    assert.equal(await child.isVisible(), true);
    assert.equal(await child.locator('.cc-details').isVisible(), true);
    await page.screenshot({ animations: 'disabled', path: path.join(directory, `compiled-expanded-${width}-${colorScheme}.png`) });
    if (!before) {
      assert.equal(await child.locator('.cc-context-name').textContent(), await child.locator('.cc-name').textContent());
      assert.equal(await child.locator('.cc-context-meta').textContent(), await child.locator('.cc-meta').textContent());
      assert.equal(await child.locator('.cc-context-meta').evaluate(node => node.scrollWidth <= node.clientWidth + 1), true);
    }
    const route = await page.evaluate(() => history.state.id);
    await child.locator('.cc-name').click();
    await page.waitForFunction(parent => history.state.id !== parent, route);
    await page.goBack();
    await page.waitForFunction(parent => history.state.id === parent, route);
    await page.locator('.child-card').first().waitFor();
    const audit = await auditText(page);
    assert.equal(audit.smallCount, 0, JSON.stringify(audit.small));
    assert.equal(audit.lowCount, 0, JSON.stringify(audit.low));
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    assert.deepEqual(errors, []);
    observations.push({ width, colorScheme, before, compiledExecutable: true, frontendSubstitution: false, keyboardDisclosure: true, directChildNavigationAndBack: true, textAudit: audit });
    await context.close();
  }
} finally { await browser.close(); for (const server of servers) server.kill(); }
await fs.writeFile(path.join(out, 'compiled-observations.json'), JSON.stringify({ baseline: 'e9dba7cac8a7b6f36f936ba77361a9fc134737f8', cases: observations }, null, 2) + '\n');
console.log('12 matched compiled Semon cases: expanded identity, keyboard, child navigation/Back, text and overflow passed');

import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';
const root = fileURLToPath(new URL('../../../../', import.meta.url));
const { chromium } = await import(pathToFileURL(path.join(root, 'tests/ui/node_modules/playwright/index.mjs')));
const output = path.join(root, 'docs/design/session-inspection/surface-refinement');
const address = (await fs.readFile(process.env.SEMON_VIEWER_LOG || '/workspace/scratch/viewer-run.log', 'utf8')).match(/http:\/\/127\.0\.0\.1:8081\/\?t=[a-f0-9]+/)[0];
const browser = await chromium.launch();
const evidence = [];
await fs.mkdir(output, { recursive: true });
try {
  for (const phase of ['before', 'after']) {
    const css = phase === 'before' ? execFileSync('git', ['show', 'bcc0a7624e753a818665518837d050af13395bef:crates/semon-sessions/src/viewer.css'], { cwd: root }) : await fs.readFile(path.join(root, 'crates/semon-sessions/src/viewer.css'));
    for (const width of [390, 1280]) for (const colorScheme of ['light', 'dark']) {
      const page = await browser.newPage({ viewport: { width, height: 860 }, deviceScaleFactor: 2, colorScheme, reducedMotion: 'reduce', hasTouch: width === 390 });
      const errors = [];
      page.on('pageerror', error => errors.push(error.message));
      await page.route(/\/viewer\.css(?:\?|$)/, route => route.fulfill({ contentType: 'text/css', body: css }));
      await page.goto(address.replace('/?t=', '/s/claude/harbor?t=') + '&compat=1');
      await page.locator('#page .child-card').first().waitFor({ state: 'attached' });
      await page.evaluate(() => document.fonts.ready);
      await page.evaluate(() => { document.querySelector('.main').scrollTop = 0; });
      const features = await page.locator('#page').evaluate(node => Object.fromEntries(['.msg.user', '.msg.assistant', '.thought', '.tsum', '.child-card', '.event'].map(selector => [selector, node.querySelectorAll(selector).length])));
      assert(Object.entries(features).filter(([selector]) => selector !== '.event').every(([,count]) => count > 0), JSON.stringify(features));
      const geometry = await page.locator('#page .msg.user, #page .thought, #page .tsum, #page .child-card, #page .event').evaluateAll(nodes => nodes.map(node => { const rect = node.getBoundingClientRect(); const css = getComputedStyle(node); return { type: node.className, x: rect.x, y: rect.y, width: rect.width, height: rect.height, font: css.fontSize }; }));
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
      await page.screenshot({ path: path.join(output, `${phase}-viewer-${width}-${colorScheme}.png`) });
      const child = page.locator('#page .child-card').first();
      const childAction = child.getByRole('button').first();
      await childAction.focus();
      assert.equal(await childAction.evaluate(n => n === document.activeElement), true);
      assert.notEqual(await childAction.evaluate(n => getComputedStyle(n).outlineStyle), 'none');
      await child.screenshot({ path: path.join(output, `${phase}-linked-session-${width}-${colorScheme}.png`) });
      for (const summary of await page.locator('#page .tsum[aria-expanded=false]').all()) await summary.evaluate(node => node.click());
      const failed = page.locator('#page .step.err:visible').first();
      await failed.waitFor();
      const disclosure = failed.locator(':scope > button');
      if (await disclosure.getAttribute('aria-expanded') !== 'true') await disclosure.click();
      assert.equal(await disclosure.getAttribute('aria-expanded'), 'true');
      await failed.scrollIntoViewIfNeeded();
      await page.screenshot({ path: path.join(output, `${phase}-failure-${width}-${colorScheme}.png`) });
      assert.deepEqual(errors, []);
      evidence.push({ phase, width, colorScheme, features, geometry, failedToolExpanded: true, keyboardChildFocusVisible: true, pageOverflow: false, cssSha256: crypto.createHash('sha256').update(css).digest('hex') });
      await page.close();
    }
  }
  for (const after of evidence.filter(item => item.phase === 'after')) {
    const before = evidence.find(item => item.phase === 'before' && item.width === after.width && item.colorScheme === after.colorScheme);
    assert.deepEqual(after.features, before.features);
    assert.deepEqual(after.geometry, before.geometry);
  }
  await fs.writeFile(path.join(output, 'observations.json'), JSON.stringify({ note: 'Same synthetic full compatibility reader and embedded controller in both phases. Only authored viewer CSS is routed. Equal DOM counts and block bounds verify that surface refinement does not reflow the reader.', cases: evidence }, null, 2) + '\n');
  console.log(JSON.stringify({ cases: evidence.length, screenshots: evidence.length * 3, geometryPreserved: true }));
} finally { await browser.close(); }

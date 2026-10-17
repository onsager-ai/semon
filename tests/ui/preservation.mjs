// Required actual-application comparison, independent of the mockup ratchets.
// The reference binary is the exact gitlink of the captured compat=1 UI.
// Pause removals are never painted back into either UI or silently exempted.
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';
import { ENV, launch, served, goto, data } from './lib.mjs';
import {
  assessPreservation,
  BEFORE,
  REFERENCE,
  THRESHOLD,
  MAX_RATIO,
  WIDTHS,
  SCHEMES,
} from './preservation-contract.mjs';

const args = process.argv.slice(2);
const historical = args.length === 1 && args[0] === '--historical-before';
if (
  (args.length && !historical) ||
  (historical && process.env.SEMON_PRESERVATION_CANDIDATE !== BEFORE)
)
  throw new Error('Only the pinned before candidate may use --historical-before');

const out = path.join(ENV.out, 'preservation');
fs.mkdirSync(out, { recursive: true });
const report = {
  candidate: process.env.SEMON_PRESERVATION_CANDIDATE ?? process.env.GITHUB_SHA,
  reference: REFERENCE.source,
  referenceHub: REFERENCE.hub,
  mode: historical ? 'historical-before' : 'required',
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  results: [],
  errors: [],
};
let browser;
async function snapshot(page) {
  return page.evaluate(() => {
    const selectors = [
      '#sidebar',
      '#topbar',
      '#lanes',
      '#page',
      '.nrow',
      '.think',
      '.tsum',
      '.session-dock',
      'dialog[open]',
    ];
    return Object.fromEntries(
      selectors.map((selector) => [
        selector,
        [...document.querySelectorAll(selector)].slice(0, 100).map((node) => {
          const box = node.getBoundingClientRect(),
            css = getComputedStyle(node);
          return {
            text: node.textContent.slice(0, 12000),
            box: [box.x, box.y, box.width, box.height],
            font: css.font,
            padding: css.padding,
            gap: css.gap,
            icons: [...node.querySelectorAll('img')].map((image) => image.getAttribute('src')),
            paths: [...node.querySelectorAll('path')].map((p) => p.getAttribute('d')),
          };
        }),
      ]),
    );
  });
}
async function capture(candidate, reference, key) {
  const images = [];
  for (const [name, page] of [
    ['candidate', candidate],
    ['reference', reference],
  ]) {
    // Both readers open at the latest record. Cancel that initial following
    // transaction with the same real reader input before comparing their heads.
    await page.mouse.move(page.viewportSize().width - 20, 300);
    await page.mouse.wheel(0, -100000);
    await page.keyboard.press('Home');
    await page.evaluate(async () => {
      await document.fonts.ready;
      window.scrollTo(0, 0);
      document.querySelector('#main').scrollTop = 0;
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    });
    await page.waitForTimeout(150);
    await page.waitForFunction(
      () => scrollY <= 1 && document.querySelector('#main').scrollTop <= 1,
    );
    const bytes = await page.screenshot({ fullPage: false, animations: 'disabled', caret: 'hide' });
    fs.writeFileSync(path.join(out, key + '-' + name + '.png'), bytes);
    fs.writeFileSync(
      path.join(out, key + '-' + name + '.json'),
      JSON.stringify(await snapshot(page), null, 2) + '\n',
    );
    images.push(PNG.sync.read(bytes));
  }
  const result = compare(...images, report.threshold);
  fs.writeFileSync(path.join(out, key + '-diff.png'), PNG.sync.write(result.diff));
  report.results.push({
    key,
    pixels: result.pixels,
    ratio: result.ratio,
    size: result.size,
    pass: !result.size && result.ratio <= report.maxRatio,
  });
}
try {
  if (!ENV.base || !process.env.SEMON_PRESERVATION_BASE)
    throw new Error('Both candidate and selected-reference servers are required');
  if (ENV.base === process.env.SEMON_PRESERVATION_BASE.replace(/\/$/, ''))
    throw new Error('Candidate and selected-reference servers must be independent');
  browser = await launch();
  const D = await data();
  for (const width of WIDTHS)
    for (const scheme of SCHEMES) {
      const dark = scheme === 'dark';
      const options = {
        size: width === 390 ? 'phone' : 'desktop',
        viewport: { width, height: width === 390 ? 844 : 860 },
        dark,
      };
      const candidate = await served(browser, options);
      const retired = [];
      candidate.on('request', (request) => {
        if (/\/api\/(model|transcript|expand)(?:[/?]|$)/.test(request.url()))
          retired.push(new URL(request.url()).pathname);
      });
      const reference = await served(browser, {
        ...options,
        base: process.env.SEMON_PRESERVATION_BASE,
        token: process.env.SEMON_PRESERVATION_TOKEN,
        path: '/?compat=1',
      });
      const prefix = width + '-' + (dark ? 'dark' : 'light');
      for (const route of [
        { v: 'home' },
        { v: 'sessions' },
        { v: 'session', id: 'harbor' },
        { v: 'session', id: 'atlas-ingest' },
        { v: 'session', id: 'deps' },
        { v: 'session', id: 'h-failed' },
        { v: 'analytics' },
      ]) {
        await goto(candidate, route, D);
        await reference.evaluate((route) => {
          history.pushState(route, '');
          dispatchEvent(new PopStateEvent('popstate', { state: route }));
        }, route);
        await reference.waitForFunction(
          (route) =>
            history.state.v === route.v &&
            !!document.querySelector('#topbar .t') &&
            (route.v !== 'session' || !!document.querySelector('#page .transcript')),
          route,
        );
        if (route.v === 'analytics')
          await reference.waitForSelector('.analytics-metrics[data-analytics-ready]');
        await capture(
          candidate,
          reference,
          prefix + '-' + route.v + (route.id ? '-' + route.id : ''),
        );
        if (route.id === 'harbor') {
          for (const page of [candidate, reference])
            await page.locator('.think-disclosure').first().click();
          await capture(candidate, reference, prefix + '-harbor-thinking-open');
          for (const page of [candidate, reference]) await page.locator('.tsum').first().click();
          await capture(candidate, reference, prefix + '-harbor-tools-open');
          for (const page of [candidate, reference]) {
            await page.locator('.tgroup .steps .step > button').first().click();
            await page.locator('.tgroup .steps .out:not([hidden])').first().waitFor();
          }
          await capture(candidate, reference, prefix + '-harbor-tool-output-open');
        }
      }
      if (width === 390) {
        await goto(candidate, { v: 'home' }, D);
        await reference.evaluate(() => {
          history.pushState({ v: 'home' }, '');
          dispatchEvent(new PopStateEvent('popstate', { state: { v: 'home' } }));
        });
        for (const page of [candidate, reference]) await page.click('#lead-btn');
        await capture(candidate, reference, prefix + '-drawer');
      }
      report.errors.push(
        ...candidate.errors.map((error) => prefix + ' candidate: ' + error),
        ...reference.errors.map((error) => prefix + ' reference: ' + error),
        ...retired.map((url) => prefix + ' retired candidate request: ' + url),
      );
      await candidate.context().close();
      await reference.context().close();
    }
} catch (error) {
  report.errors.push(error.stack);
} finally {
  await browser?.close();
  report.failures = assessPreservation(report);
  fs.writeFileSync(path.join(out, 'report.json'), JSON.stringify(report, null, 2) + '\n');
}
console.log(JSON.stringify(report));
// Only the fixed historical before-run is diagnostic. The candidate must satisfy
// pixel, geometry, provenance and exact coverage assertions. Neither native
// qualification nor the control pause grants an implicit pixel waiver.
if (report.failures.coverage.length || (!historical && report.failures.visual.length))
  process.exitCode = 1;

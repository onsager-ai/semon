// Actual application comparison, separate from the unchanged mockup ratchets.
// The reference binary is the exact gitlink of the captured compat=1 UI.
// Differences remain reported; pause removals are never painted back into either UI.
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';
import { ENV, launch, served, goto, data } from './lib.mjs';

const out = path.join(ENV.out, 'preservation');
fs.mkdirSync(out, { recursive: true });
const report = {
  candidate: process.env.SEMON_PRESERVATION_CANDIDATE ?? process.env.GITHUB_SHA,
  reference: '263a02da278716ab2309b8e34a463419e5c781e5',
  threshold: 0.1,
  maxRatio: 0.0001,
  results: [],
  errors: [],
};
const browser = await launch();
const D = await data();
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
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
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
  await browser.close();
  fs.writeFileSync(path.join(out, 'report.json'), JSON.stringify(report, null, 2) + '\n');
}
console.log(JSON.stringify(report));
// Exact matching is diagnostic because the explicit pause removes controls.
// Missing captures and execution errors still fail; no reference or gate changes.
if (report.results.length !== 38 || report.errors.length) process.exitCode = 1;

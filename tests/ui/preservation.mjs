// Required 62-state actual-application comparison, independent of the historical-frame gate.
// The reference binary is the exact gitlink of the captured compat=1 UI.
// Pause removals are never painted back into either UI or silently exempted.
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';
import { ENV, launch, served, goto, data } from './lib.mjs';
import { compareDomEvidence, recordDomDiffSummary } from './dom-evidence.mjs';
import {
  assessPreservation,
  assessVisualAcceptances,
  BEFORE,
  REFERENCE,
  THRESHOLD,
  MAX_RATIO,
  WIDTHS,
  SCHEMES,
  sha256,
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
  candidateFingerprint: null,
  reference: REFERENCE.source,
  referenceHub: REFERENCE.hub,
  mode: historical ? 'historical-before' : 'required',
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  results: [],
  errors: [],
  domDiffSummary: { casesCompared: 0, casesWithChanges: 0, bySelector: {} },
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
    return {
      historyState: history.state,
      pathname: location.pathname,
      selectors: Object.fromEntries(
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
      ),
    };
  });
}
async function capture(candidate, reference, key) {
  const images = [],
    artifactDigests = {},
    evidenceBySide = {},
    artifactPaths = {
      candidateImage: `preservation/${key}-candidate.png`,
      candidateEvidence: `preservation/${key}-candidate.json`,
      referenceImage: `preservation/${key}-reference.png`,
      referenceEvidence: `preservation/${key}-reference.json`,
      diffImage: `preservation/${key}-diff.png`,
      comparisonEvidence: `preservation/${key}-comparison.json`,
    };
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
    evidenceBySide[name] = await snapshot(page);
    const evidence = Buffer.from(JSON.stringify(evidenceBySide[name], null, 2) + '\n');
    fs.writeFileSync(path.join(out, key + '-' + name + '.json'), evidence);
    artifactDigests[name] = { image: sha256(bytes), evidence: sha256(evidence) };
    images.push(PNG.sync.read(bytes));
  }
  recordDomDiffSummary(
    report.domDiffSummary,
    compareDomEvidence(evidenceBySide.candidate, evidenceBySide.reference, key),
  );
  const result = compare(...images, report.threshold);
  const diffBytes = PNG.sync.write(result.diff);
  fs.writeFileSync(path.join(out, key + '-diff.png'), diffBytes);
  const comparisonEvidence = Buffer.from(
    JSON.stringify(
      {
        key,
        pixels: result.pixels,
        ratio: result.ratio,
        size: result.size,
        threshold: report.threshold,
        maxRatio: report.maxRatio,
      },
      null,
      2,
    ) + '\n',
  );
  fs.writeFileSync(path.join(out, key + '-comparison.json'), comparisonEvidence);
  artifactDigests.diff = { image: sha256(diffBytes), evidence: sha256(comparisonEvidence) };
  report.results.push({
    key,
    pixels: result.pixels,
    ratio: result.ratio,
    size: result.size,
    pass: !result.size && result.ratio <= report.maxRatio,
    artifactDigests,
    artifactPaths,
  });
}
try {
  if (!ENV.base || !process.env.SEMON_PRESERVATION_BASE || !process.env.SEMON_PRESERVATION_TOKEN)
    throw new Error('Both candidate and selected-reference servers are required');
  if (ENV.base === process.env.SEMON_PRESERVATION_BASE.replace(/\/$/, ''))
    throw new Error('Candidate and selected-reference servers must be independent');
  browser = await launch();
  if (!/^[a-f0-9]{40}$/.test(report.candidate ?? ''))
    throw new Error('Candidate commit SHA is required for exact visual evidence');
  const candidateBinary = process.env.SEMON_VISUAL_CANDIDATE_BIN ?? process.env.SEMON_BIN;
  if (!candidateBinary || !fs.existsSync(candidateBinary))
    throw new Error('Candidate binary is required for exact visual evidence');
  report.candidateFingerprint = sha256(fs.readFileSync(candidateBinary));
  const D = await data();
  for (const width of WIDTHS)
    for (const scheme of SCHEMES) {
      const dark = scheme === 'dark';
      const options = {
        size: width === 390 ? 'phone' : 'desktop',
        viewport: { width, height: width === 390 ? 844 : 860 },
        dark,
      };
      const candidate = await served(browser, { ...options, captureRequests: true });
      const retired = candidate.requests ?? [];
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
        ...retired
          .filter((url) =>
            /^\/api\/(?:model|transcript|expand|tx|tool|image)(?:\/|$)/.test(new URL(url).pathname),
          )
          .map((url) => prefix + ' retired candidate request: ' + new URL(url).pathname),
      );
      await candidate.context().close();
      await reference.context().close();
    }
} catch (error) {
  report.errors.push(error.stack ?? error.message);
} finally {
  await browser?.close();
  report.failures = assessPreservation(report);
  report.acceptance = assessVisualAcceptances(report, 'preservation', report.failures);
  fs.writeFileSync(path.join(out, 'report.json'), JSON.stringify(report, null, 2) + '\n');
}
console.log(JSON.stringify(report));
// Only the fixed historical before-run is diagnostic. The candidate must satisfy
// pixel, geometry, provenance and exact coverage assertions. Neither native
// qualification nor the control pause grants an implicit pixel waiver.
if (report.failures.coverage.length || (!historical && !report.acceptance.pass))
  process.exitCode = 1;

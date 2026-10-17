// Required 62-state actual-application comparison, independent of the historical-frame gate.
// The reference binary is the exact gitlink of the captured compat=1 UI.
// Pause removals are never painted back into either UI or silently exempted.
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';
import { ENV, launch, served, goto, data, waitForCaptureReady } from './lib.mjs';
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
  apiDiagnostics: [],
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
async function snapshotChromeDiagnostics(page) {
  return page.evaluate(() => {
    const pseudo = (node, which) => {
      const style = getComputedStyle(node, which);
      if (style.display === 'none' || style.content === 'none' || style.content === 'normal')
        return null;
      return {
        content: style.content.slice(0, 120),
        display: style.display,
        color: style.color,
        background: style.backgroundColor,
        backgroundImage: style.backgroundImage,
        border: style.borderColor,
        opacity: style.opacity,
        filter: style.filter,
        transform: style.transform,
        width: style.width,
        height: style.height,
      };
    };
    const describe = (node) => {
      const box = node.getBoundingClientRect(),
        css = getComputedStyle(node);
      return {
        tag: node.tagName.toLowerCase(),
        id: node.id,
        className: node.getAttribute('class') ?? '',
        role: node.getAttribute('role'),
        text: (node.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 160),
        ariaLabel: node.getAttribute('aria-label'),
        ariaExpanded: node.getAttribute('aria-expanded'),
        title: node.getAttribute('title'),
        box: [box.x, box.y, box.width, box.height],
        style: {
          display: css.display,
          visibility: css.visibility,
          position: css.position,
          color: css.color,
          background: css.backgroundColor,
          backgroundImage: css.backgroundImage,
          border: css.borderColor,
          outline: [css.outlineStyle, css.outlineWidth, css.outlineColor],
          opacity: css.opacity,
          font: css.font,
          fontWeight: css.fontWeight,
          fill: css.fill,
          stroke: css.stroke,
          strokeWidth: css.strokeWidth,
          filter: css.filter,
          transform: css.transform,
          textShadow: css.textShadow,
          boxShadow: css.boxShadow,
        },
        state: {
          hidden: !!node.hidden,
          disabled: node.matches(':disabled'),
          hovered: node.matches(':hover'),
          active: node.matches(':active'),
          focused: node.matches(':focus'),
          focusVisible: node.matches(':focus-visible'),
          focusWithin: node.matches(':focus-within'),
        },
        pseudo: { before: pseudo(node, '::before'), after: pseudo(node, '::after') },
      };
    };
    const active = document.activeElement;
    const transcript = document.querySelector('.transcript');
    const transcriptBox = transcript?.getBoundingClientRect();
    const dockBox = document.querySelector('.session-dock')?.getBoundingClientRect();
    const transcriptStyle = transcript ? getComputedStyle(transcript) : null;
    return {
      activeElement: active ? describe(active) : null,
      viewport: {
        width: innerWidth,
        height: innerHeight,
        devicePixelRatio,
        scrollX,
        scrollY,
        mainScrollTop: document.querySelector('#main')?.scrollTop ?? 0,
      },
      transcriptClip:
        transcriptBox && transcriptStyle
          ? {
              box: [transcriptBox.x, transcriptBox.y, transcriptBox.width, transcriptBox.height],
              bottom: transcriptBox.bottom,
              dockTop: dockBox?.top ?? null,
              clipPath: transcriptStyle.clipPath,
              readingClip: transcriptStyle.getPropertyValue('--reading-clip').trim(),
            }
          : null,
      roots: Object.fromEntries(
        ['#topbar', '.session-dock'].map((selector) => {
          const root = document.querySelector(selector),
            nodes = root ? [root, ...root.querySelectorAll('*')].slice(0, 100) : [];
          return [selector, nodes.map(describe)];
        }),
      ),
    };
  });
}
async function capture(candidate, reference, key) {
  const images = [],
    artifactDigests = {},
    evidenceBySide = {},
    pages = [
      ['candidate', candidate],
      ['reference', reference],
    ],
    artifactPaths = {
      candidateImage: `preservation/${key}-candidate.png`,
      candidateEvidence: `preservation/${key}-candidate.json`,
      referenceImage: `preservation/${key}-reference.png`,
      referenceEvidence: `preservation/${key}-reference.json`,
      diffImage: `preservation/${key}-diff.png`,
      comparisonEvidence: `preservation/${key}-comparison.json`,
    };
  const readiness = await Promise.allSettled(
    pages.map(async ([name, page]) => {
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
      await page.waitForFunction(
        () => scrollY <= 1 && document.querySelector('#main').scrollTop <= 1,
      );
      await waitForCaptureReady(page, `${key} ${name}`);
    }),
  );
  for (const [name, page] of pages) {
    const from = page.captureApiEventsReported ?? 0;
    report.apiDiagnostics.push(
      ...page.captureApiEvents.slice(from).map((event) => ({ state: key, side: name, ...event })),
    );
    page.captureApiEventsReported = page.captureApiEvents.length;
  }
  const readinessErrors = readiness
    .filter((result) => result.status === 'rejected')
    .map((result) => result.reason?.message ?? String(result.reason));
  if (readinessErrors.length)
    throw new Error(`${key} capture readiness failed: ${readinessErrors.join(' | ')}`);
  for (const [name, page] of pages) {
    const bytes = await page.screenshot({ fullPage: false, animations: 'disabled', caret: 'hide' });
    fs.writeFileSync(path.join(out, key + '-' + name + '.png'), bytes);
    evidenceBySide[name] = await snapshot(page);
    const evidence = Buffer.from(JSON.stringify(evidenceBySide[name], null, 2) + '\n');
    fs.writeFileSync(path.join(out, key + '-' + name + '.json'), evidence);
    artifactDigests[name] = { image: sha256(bytes), evidence: sha256(evidence) };
    images.push(PNG.sync.read(bytes));
  }
  const includeChromeDiagnostics =
    /^(390|820)-(light|dark)-(?:session-harbor|harbor-tools-open|harbor-tool-output-open)$/.test(
      key,
    );
  if (includeChromeDiagnostics) {
    for (const [name, page] of pages) {
      evidenceBySide[name].chromeDiagnostics = await snapshotChromeDiagnostics(page);
      const evidence = Buffer.from(JSON.stringify(evidenceBySide[name], null, 2) + '\n');
      fs.writeFileSync(path.join(out, key + '-' + name + '.json'), evidence);
      artifactDigests[name].evidence = sha256(evidence);
    }
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
        difference: result.difference,
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
    difference: result.difference,
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
      const candidate = await served(browser, {
        ...options,
        captureRequests: true,
        captureReadiness: true,
        captureRole: 'candidate',
      });
      const retired = candidate.requests ?? [];
      const reference = await served(browser, {
        ...options,
        captureRequests: true,
        captureReadiness: true,
        captureRole: 'reference',
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
        reference.captureRouteKey = JSON.stringify(route);
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
        reference.captureRouteKey = JSON.stringify({ v: 'home' });
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
      for (const [side, page] of [
        ['candidate', candidate],
        ['reference', reference],
      ]) {
        const from = page.captureApiEventsReported ?? 0;
        report.apiDiagnostics.push(
          ...page.captureApiEvents.slice(from).map((event) => ({ state: prefix, side, ...event })),
        );
        page.captureApiEventsReported = page.captureApiEvents.length;
        if (page.captureApiEventsTruncated)
          report.errors.push(
            `${prefix} ${side} API diagnostics truncated after ${page.captureApiEventsTruncated} events`,
          );
      }
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
console.log(
  JSON.stringify({
    candidate: report.candidate,
    reference: report.reference,
    cases: report.results.length,
    coverageFailures: report.failures.coverage.length,
    visualFailures: report.failures.visual.length,
    acceptedVisualFailures: report.acceptance.accepted.length,
    pendingVisualFailures: report.acceptance.pending.length,
    apiEvents: report.apiDiagnostics.length,
    apiFailures: report.apiDiagnostics.filter(
      (event) =>
        event.event === 'failed' ||
        (event.event === 'response' && !event.ok && event.status !== 304),
    ).length,
    errors: report.errors.length,
  }),
);
// Only the fixed historical before-run is diagnostic. The candidate must satisfy
// pixel, geometry, provenance and exact coverage assertions. Neither native
// qualification nor the control pause grants an implicit pixel waiver.
if (report.failures.coverage.length || (!historical && !report.acceptance.pass))
  process.exitCode = 1;

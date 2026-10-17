// CI-only observation of the unchanged preservation journey and two fixed
// trusted-scroll interventions. This is supporting evidence, never acceptance.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import {
  ENV,
  data,
  goto,
  homePostTagDiagnostics,
  launch,
  served,
  waitForCaptureReady,
} from '../lib.mjs';
import { compare } from '../pixel-diff.mjs';
import { MAX_RATIO, REFERENCE, THRESHOLD, sha256 } from '../preservation-contract.mjs';

const DEADLINE = 8000; // Existing capture-readiness deadline; no product timers change.
const FORWARD = 160;
const BACK = -100000;
const CASES = [
  [820, 'light'],
  [820, 'dark'],
  [1280, 'light'],
  [1280, 'dark'],
];
const out = path.join(ENV.out, 'harbor-expansion-scroll');
fs.mkdirSync(out, { recursive: true });
const report = {
  diagnosticOnly: true,
  acceptance: false,
  candidate: process.env.SEMON_PRESERVATION_CANDIDATE ?? process.env.GITHUB_SHA,
  diagnosticCheckout: process.env.GITHUB_SHA ?? null,
  candidateRuntimeSource: process.env.SEMON_DIAGNOSTIC_CANDIDATE_SOURCE ?? null,
  binaryArtifactRun: process.env.SEMON_DIAGNOSTIC_ARTIFACT_RUN ?? null,
  reference: REFERENCE,
  candidateFingerprint: null,
  referenceFingerprint: null,
  clock: ENV.now,
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  protocol: {
    cases: CASES,
    forwardWheel: FORWARD,
    returnWheel: BACK,
    interventions: ['after Tools', 'after first tool output'],
    outputJourney:
      'Output follows the recorded Tools intervention; it is not the original baseline.',
    originalBaseline:
      'Preserve run 39075952251 separately; this script never replaces its artifacts.',
    limits: { events: 512, mutations: 128, geometrySamples: 32 },
  },
  comparisons: [],
  cases: [],
  errors: [],
};
const safeError = (error) =>
  String(error?.message ?? error)
    .replace(/https?:\/\/[^\s"'<>]+/g, (url) => {
      try {
        const parsed = new URL(url);
        return parsed.origin + parsed.pathname;
      } catch {
        return '[URL redacted]';
      }
    })
    .replace(/\?[^\s"'<>]*/g, '?[redacted]')
    .slice(0, 2000);
const writeJson = (name, value) => {
  fs.writeFileSync(path.join(out, name), JSON.stringify(value, null, 2) + '\n');
};
async function bounded(promise, label) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(label + ' exceeded the capture deadline')),
          DEADLINE,
        );
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function installObserver(page) {
  await page.evaluate(() => {
    const events = [],
      mutations = [],
      samples = [];
    let stage = 'installed',
      sequence = 0,
      eventsTruncated = 0,
      mutationsTruncated = 0;
    const identity = (node) =>
      node instanceof Element
        ? {
            tag: node.tagName.toLowerCase(),
            id: node.id.slice(0, 80),
            className: (node.getAttribute('class') ?? '').slice(0, 160),
          }
        : { tag: node === window ? 'window' : 'document' };
    const route = () => {
      const result = {};
      for (const key of ['v', 'id', 'sheet', 'scrollTop']) {
        const value = history.state?.[key];
        if (typeof value === 'string') result[key] = value.slice(0, 128);
        else if (typeof value === 'number' && Number.isFinite(value)) result[key] = value;
      }
      return result;
    };
    const state = () => {
      const main = document.querySelector('#main'),
        jump = document.querySelector('.jump-wrap'),
        tip = document.querySelector('#sh-tooltip');
      return {
        mainTop: main?.scrollTop ?? null,
        windowTop: scrollY,
        jumpHidden: jump?.hidden ?? null,
        tipHidden: tip?.hidden ?? null,
      };
    };
    const record = (event) => {
      const entry = {
        sequence: ++sequence,
        at: performance.now(),
        stage,
        type: event.type,
        trusted: event.isTrusted,
        target: identity(event.target),
        ...state(),
      };
      if (event instanceof WheelEvent)
        Object.assign(entry, { deltaY: event.deltaY, x: event.clientX, y: event.clientY });
      if (event instanceof KeyboardEvent) entry.key = event.key;
      if (events.length < 512) events.push(entry);
      else eventsTruncated++;
    };
    for (const type of ['wheel', 'scroll', 'keydown', 'pointerdown', 'focusin', 'popstate'])
      window.addEventListener(type, record, { capture: true, passive: true });
    const relevant = '#jump-bottom, .jump-wrap, .session-jump-slot, #sh-tooltip';
    const observer = new MutationObserver((changes) => {
      for (const change of changes) {
        const related =
          change.target instanceof Element &&
          (change.target.matches(relevant) ||
            (change.type === 'childList' &&
              [...change.addedNodes, ...change.removedNodes].some(
                (node) =>
                  node instanceof Element &&
                  (node.matches(relevant) || !!node.querySelector(relevant)),
              )));
        if (!related) continue;
        const entry = {
          sequence: ++sequence,
          at: performance.now(),
          stage,
          type: change.type,
          target: identity(change.target),
          attribute: change.attributeName,
          before: change.oldValue?.slice(0, 160) ?? null,
          after:
            change.type === 'attributes'
              ? (change.target.getAttribute(change.attributeName)?.slice(0, 160) ?? null)
              : null,
          ...state(),
        };
        if (mutations.length < 128) mutations.push(entry);
        else mutationsTruncated++;
      }
    });
    observer.observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeOldValue: true,
      attributeFilter: ['hidden', 'class', 'style'],
    });
    const rect = (node) => {
      if (!node) return null;
      const box = node.getBoundingClientRect();
      return [box.x, box.y, box.width, box.height];
    };
    const describe = (selector) => {
      const node = document.querySelector(selector);
      if (!node) return null;
      const css = getComputedStyle(node);
      return {
        ...identity(node),
        box: rect(node),
        hidden: node.hidden,
        display: css.display,
        overflowY: css.overflowY,
        clipPath: css.clipPath,
        text: selector === '#sh-tooltip' ? node.textContent.slice(0, 300) : undefined,
      };
    };
    window.__harborScrollDiagnostic = {
      setStage(value) {
        stage = value;
      },
      witness() {
        return {
          ...state(),
          lastTrustedWheel:
            events.findLast((event) => event.type === 'wheel' && event.trusted) ?? null,
        };
      },
      sample(label) {
        if (samples.length >= 32) throw new Error('Geometry sample limit exceeded');
        const main = document.querySelector('#main'),
          active = document.activeElement,
          tip = document.querySelector('#sh-tooltip'),
          result = {
            label,
            sequence,
            at: performance.now(),
            route: route(),
            viewport: [innerWidth, innerHeight],
            active: identity(active),
            main: {
              top: main?.scrollTop ?? null,
              height: main?.scrollHeight ?? null,
              viewport: main?.clientHeight ?? null,
              gap: main
                ? Math.max(0, main.scrollHeight - main.clientHeight - main.scrollTop)
                : null,
            },
            windowTop: scrollY,
            roots: Object.fromEntries(
              [
                '#main',
                '.transcript',
                '.session-dock',
                '.jump-wrap',
                '#jump-bottom',
                '#sh-tooltip',
              ].map((selector) => [selector, describe(selector)]),
            ),
            tooltip: {
              hidden: tip?.hidden ?? null,
              text: tip?.textContent?.slice(0, 300) ?? null,
              box: rect(tip),
              popoverOpen: tip?.hasAttribute('popover') ? tip.matches(':popover-open') : null,
            },
            lastTrustedWheel:
              events.findLast((event) => event.type === 'wheel' && event.trusted) ?? null,
            lastScroll: events.findLast((event) => event.type === 'scroll') ?? null,
          };
        samples.push(result);
        return result;
      },
      collect() {
        observer.disconnect();
        for (const type of ['wheel', 'scroll', 'keydown', 'pointerdown', 'focusin', 'popstate'])
          window.removeEventListener(type, record, true);
        return { events, mutations, samples, eventsTruncated, mutationsTruncated };
      },
    };
  });
}
const stage = (page, value) =>
  page.evaluate((value) => window.__harborScrollDiagnostic.setStage(value), value);
const sample = (page, label) =>
  page.evaluate((label) => window.__harborScrollDiagnostic.sample(label), label);

async function ready(page, label) {
  await page.waitForFunction(
    () => {
      const panel = document.querySelector('#page');
      return (
        history.state?.v &&
        document.querySelector('#topbar .t, #topbar #find') &&
        panel &&
        !panel.hasAttribute('aria-busy') &&
        !panel.querySelector('[aria-busy="true"], [data-loading="true"]') &&
        (history.state.v !== 'session' ||
          (panel.querySelector("section[aria-label='Transcript']") &&
            panel.querySelector('[data-entry-key]')))
      );
    },
    null,
    { timeout: DEADLINE },
  );
  await page.evaluate(() => document.fonts.ready);
  await waitForCaptureReady(page, label);
}
// This is the existing preservation capture input, including its potentially
// no-op upward wheel at scroll zero. Keep it intact as the pre-intervention arm.
async function originalReset(page, label) {
  await stage(page, label + ':original-reset');
  await ready(page, label + ':before reset');
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
    null,
    { timeout: DEADLINE },
  );
  await waitForCaptureReady(page, label + ':after reset');
}
async function capture(pages, key, reset = true) {
  const images = [],
    sides = {};
  const readiness = await Promise.allSettled(
    pages.map(([side, page]) =>
      reset ? originalReset(page, key + ':' + side) : ready(page, key + ':' + side),
    ),
  );
  const failures = readiness.filter((result) => result.status === 'rejected');
  if (failures.length)
    throw new AggregateError(
      failures.map((result) => result.reason),
      key + ' readiness failed',
    );
  for (const [side, page] of pages) {
    if (key.endsWith('-filter-open')) {
      const filter = await page.evaluate(() => {
        const dialog = document.querySelector('.filters-sheet');
        return {
          open: dialog?.open === true,
          focused: document.activeElement === dialog?.querySelector('.vclose'),
          top: dialog?.querySelector('.vb')?.scrollTop,
        };
      });
      assert.deepEqual(filter, { open: true, focused: true, top: 0 });
    }
    await stage(page, key + ':screenshot');
    const before = await sample(page, key + ':before-screenshot');
    const bytes = await page.screenshot({ fullPage: false, animations: 'disabled', caret: 'hide' });
    const after = await sample(page, key + ':after-screenshot');
    fs.writeFileSync(path.join(out, key + '-' + side + '.png'), bytes);
    const evidence = { before, after };
    writeJson(key + '-' + side + '.json', evidence);
    sides[side] = {
      image: sha256(bytes),
      evidence: sha256(Buffer.from(JSON.stringify(evidence, null, 2) + '\n')),
    };
    images.push(PNG.sync.read(bytes));
  }
  const result = compare(...images, THRESHOLD),
    diff = PNG.sync.write(result.diff);
  fs.writeFileSync(path.join(out, key + '-diff.png'), diff);
  const comparison = {
    key,
    pixels: result.pixels,
    ratio: result.ratio,
    size: result.size,
    difference: result.difference,
    pass: !result.size && result.ratio <= MAX_RATIO,
    artifacts: { ...sides, diff: sha256(diff) },
  };
  writeJson(key + '-comparison.json', comparison);
  report.comparisons.push(comparison);
}
async function filterOpen(page) {
  await stage(page, 'filter:open');
  await page.locator('.facet-btn').click();
  const dialog = page.locator('.filters-sheet');
  await page.waitForFunction(() => document.querySelector('.filters-sheet')?.open === true);
  await dialog.locator('.vclose').focus();
  await dialog.locator('.vb').evaluate((node) => {
    node.scrollTop = 0;
  });
}
async function filterClose(page) {
  await stage(page, 'filter:close');
  await page.locator('.filters-sheet .vclose').click();
  await page.waitForFunction(
    () =>
      !document.querySelector('.filters-sheet')?.open &&
      !history.state?.sheet &&
      history.state?.v === 'sessions' &&
      document.activeElement === document.querySelector('.facet-btn'),
  );
  await sample(page, 'filter:closed');
}
async function referenceGoto(page, route) {
  page.captureRouteKey = JSON.stringify(route);
  await stage(page, 'goto:' + route.v);
  await page.evaluate((route) => {
    history.pushState(route, '');
    dispatchEvent(new PopStateEvent('popstate', { state: route }));
  }, route);
  await page.waitForFunction(
    (route) =>
      history.state.v === route.v &&
      document.querySelector('#topbar .t') &&
      (route.v !== 'session' || document.querySelector('#page .transcript')),
    route,
  );
}
async function trustedScrollPair(page, label) {
  await stage(page, label + ':choose inert point');
  const before = await sample(page, label + ':before-wheel');
  const point = await page.evaluate(() => {
    const main = document.querySelector('#main'),
      box = main.getBoundingClientRect();
    const forbidden = 'a,button,input,textarea,select,summary,[role="button"],[data-tip],dialog';
    for (const x of [box.left + 8, box.right - 24])
      for (const y of [box.top + 100, box.top + box.height * 0.35, box.top + box.height * 0.5]) {
        const node = document.elementFromPoint(x, y);
        if (node && (node === main || main.contains(node)) && !node.closest(forbidden))
          return { x, y };
      }
    throw new Error('No inert point in the actual main scroller');
  });
  assert.equal(before.main.top, 0, label + ' starts at top');
  await stage(page, label + ':trusted-forward');
  await page.mouse.move(point.x, point.y);
  await page.mouse.wheel(0, FORWARD);
  await page.waitForFunction(
    ({ label, delta }) => {
      const wheel = window.__harborScrollDiagnostic.witness().lastTrustedWheel;
      return wheel?.stage === label + ':trusted-forward' && wheel.deltaY === delta;
    },
    { label, delta: FORWARD },
    { timeout: DEADLINE },
  );
  if (before.main.height > before.main.viewport)
    await page.waitForFunction(() => document.querySelector('#main').scrollTop > 0, null, {
      timeout: DEADLINE,
    });
  else
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    );
  const advanced = await sample(page, label + ':after-forward');
  assert.equal(advanced.lastTrustedWheel?.deltaY, FORWARD, label + ' trusted positive wheel');
  assert.equal(advanced.lastTrustedWheel?.stage, label + ':trusted-forward');
  const noFlow = before.main.height <= before.main.viewport;
  if (noFlow) assert.equal(advanced.main.top, 0, label + ' explicit no-flow');
  else assert(advanced.main.top > before.main.top, label + ' actual offset advance');
  await stage(page, label + ':trusted-return');
  await page.mouse.wheel(0, BACK);
  await page.waitForFunction(
    ({ label, delta }) => {
      const wheel = window.__harborScrollDiagnostic.witness().lastTrustedWheel;
      return wheel?.stage === label + ':trusted-return' && wheel.deltaY === delta;
    },
    { label, delta: BACK },
    { timeout: DEADLINE },
  );
  await page.waitForFunction(() => document.querySelector('#main').scrollTop === 0, null, {
    timeout: DEADLINE,
  });
  await waitForCaptureReady(page, label + ':after trusted return');
  const returned = await sample(page, label + ':after-return');
  assert.equal(returned.lastTrustedWheel?.deltaY, BACK, label + ' trusted negative wheel');
  assert.equal(returned.lastTrustedWheel?.stage, label + ':trusted-return');
  if (!noFlow) assert(returned.main.top < advanced.main.top, label + ' actual return movement');
  assert.equal(returned.main.top, 0);
  assert.equal(returned.windowTop, 0);
  return { point, noFlow, before, advanced, returned };
}

let browser;
try {
  assert(
    ENV.base &&
      ENV.token &&
      process.env.SEMON_PRESERVATION_BASE &&
      process.env.SEMON_PRESERVATION_TOKEN,
  );
  assert.notEqual(ENV.base, process.env.SEMON_PRESERVATION_BASE.replace(/\/$/, ''));
  assert.match(report.candidate ?? '', /^[0-9a-f]{40}$/);
  const candidateBinary = process.env.SEMON_VISUAL_CANDIDATE_BIN ?? process.env.SEMON_BIN;
  const referenceBinary = process.env.SEMON_DIAGNOSTIC_REFERENCE_BIN;
  assert(candidateBinary && referenceBinary, 'Both immutable binary paths are required');
  report.candidateFingerprint = sha256(fs.readFileSync(candidateBinary));
  report.referenceFingerprint = sha256(fs.readFileSync(referenceBinary));
  const D = await data();
  browser = await launch();
  for (const [width, scheme] of CASES) {
    const key = width + '-' + scheme,
      result = { key, interventions: [], evidence: {} },
      pages = [];
    report.cases.push(result);
    try {
      const options = {
        size: 'desktop',
        viewport: { width, height: 860 },
        dark: scheme === 'dark',
        captureRequests: true,
        captureReadiness: true,
      };
      pages.push([
        'candidate',
        await served(browser, { ...options, captureRole: 'candidate', captureHomePostTag: true }),
      ]);
      pages.push([
        'reference',
        await served(browser, {
          ...options,
          captureRole: 'reference',
          base: process.env.SEMON_PRESERVATION_BASE,
          token: process.env.SEMON_PRESERVATION_TOKEN,
          path: '/?compat=1',
        }),
      ]);
      for (const [, page] of pages) {
        page.setDefaultTimeout(DEADLINE);
        await installObserver(page);
      }
      for (const route of [{ v: 'home' }, { v: 'sessions' }, { v: 'session', id: 'harbor' }]) {
        await stage(pages[0][1], 'goto:' + route.v);
        await goto(pages[0][1], route, D);
        await referenceGoto(pages[1][1], route);
        await capture(pages, key + '-' + route.v);
        if (route.v === 'sessions') {
          for (const [, page] of pages) await filterOpen(page);
          await capture(pages, key + '-filter-open');
          for (const [, page] of pages) await filterClose(page);
        }
      }
      for (const [, page] of pages) {
        await stage(page, 'thinking:open');
        await page.locator('.think-disclosure').first().click();
      }
      await capture(pages, key + '-thinking-open');
      for (const [, page] of pages) {
        await stage(page, 'tools:open');
        await page.locator('.tsum').first().click();
      }
      await capture(pages, key + '-tools-original');
      for (const [side, page] of pages)
        result.interventions.push({
          stage: 'tools',
          side,
          ...(await trustedScrollPair(page, 'tools')),
        });
      await capture(pages, key + '-tools-after-trusted-scroll', false);
      for (const [, page] of pages) {
        await stage(page, 'output:open after Tools intervention');
        await page.locator('.tgroup .steps .step > button').first().click();
        await page.locator('.tgroup .steps .out:not([hidden])').first().waitFor();
      }
      await capture(pages, key + '-output-original-reset-after-tools-intervention');
      for (const [side, page] of pages)
        result.interventions.push({
          stage: 'output',
          side,
          ...(await trustedScrollPair(page, 'output')),
        });
      await capture(pages, key + '-output-after-trusted-scroll', false);
    } catch (error) {
      result.error = safeError(error);
      report.errors.push({ key, error: result.error });
    } finally {
      for (const [side, page] of pages) {
        try {
          const observed = await page.evaluate(
            () => window.__harborScrollDiagnostic?.collect() ?? null,
          );
          const evidence = {
            observed,
            api: page.captureApiEvents,
            apiTruncated: page.captureApiEventsTruncated,
            pageErrors: page.errors.map(safeError),
            homePostTags: side === 'candidate' ? homePostTagDiagnostics(page) : undefined,
          };
          writeJson(key + '-' + side + '-events.json', evidence);
          result.evidence[side] = key + '-' + side + '-events.json';
          assert(observed, 'Observer must be installed');
          assert.equal(observed.eventsTruncated, 0, 'Event trace truncated');
          assert.equal(observed.mutationsTruncated, 0, 'Jump/tooltip mutation trace truncated');
          assert.equal(page.captureApiEventsTruncated, 0, 'API trace truncated');
          assert.equal(page.errors.length, 0, 'Page errors retained');
        } catch (error) {
          report.errors.push({ key, side, collectionError: safeError(error) });
        }
        try {
          await bounded(page.context().close(), key + ':' + side + ' context close');
        } catch (error) {
          report.errors.push({ key, side, cleanupError: safeError(error) });
        }
      }
      writeJson('report.json', report);
    }
  }
} catch (error) {
  report.errors.push({ setupError: safeError(error) });
} finally {
  try {
    if (browser) await bounded(browser.close(), 'browser close');
  } catch (error) {
    report.errors.push({ cleanupError: safeError(error) });
  }
  report.visualDifferences = report.comparisons.filter((result) => !result.pass).length;
  writeJson('report.json', report);
}
console.log(
  JSON.stringify({
    diagnosticOnly: true,
    cases: report.cases.length,
    comparisons: report.comparisons.length,
    visualDifferences: report.visualDifferences,
    errors: report.errors,
  }),
);
// Differences remain raw findings, regardless of whether the diagnostic ran to
// completion. No comparator/approval registry consumes this supporting report.
if (report.errors.length) process.exitCode = 1;

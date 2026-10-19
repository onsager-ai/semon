import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { PNG } from 'pngjs';
import { launch } from './lib.mjs';
import { compare } from './pixel-diff.mjs';
import { createStorybookServer } from './storybook-server.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const contract = JSON.parse(fs.readFileSync(path.join(here, 'storybook-contract.json'), 'utf8'));
const referenceMap = JSON.parse(
  fs.readFileSync(path.join(here, contract.referenceMap), 'utf8'),
);
const storyDir = path.resolve(here, 'storybook-static');
const storyIndexPath = path.join(storyDir, 'index.json');
if (!fs.existsSync(storyIndexPath)) throw new Error('Build the Storybook static site before capture');
const storyIndex = JSON.parse(fs.readFileSync(storyIndexPath, 'utf8'));
const storyIds = Object.entries(storyIndex.entries ?? {})
  .filter(([, entry]) => entry.type === 'story')
  .map(([id]) => id)
  .sort();
const pending = [];
const failures = [];
const results = [];
const output = path.resolve(
  process.env.SEMON_UI_OUT ?? path.join(here, 'out'),
  'storybook/candidates',
);
fs.mkdirSync(output, { recursive: true });

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

async function verifyAssertions(page, assertions, pageErrors, key) {
  for (const assertion of assertions ?? []) {
    const target = page.locator(assertion.selector).first();
    if (assertion.kind === 'absent') {
      if ((await page.locator(assertion.selector).count()) !== 0) {
        pageErrors.push(`${key}: unexpected element ${assertion.selector}`);
      }
      continue;
    }
    await target.waitFor({ state: 'visible', timeout: 10000 });
    if (assertion.text) {
      const visibleText = await target.innerText();
      if (!visibleText.includes(assertion.text)) {
        pageErrors.push(`${key}: ${assertion.selector} did not include expected text: ${assertion.text}`);
      }
    }
  }
}

let browser;
let server;
let browserVersion = null;
try {
  server = await createStorybookServer(storyDir);
  browser = await launch({ baselineRaster: true });
  browserVersion = browser.version();
  for (const storyId of storyIds) {
    for (const width of contract.pixelPolicy.dimensions) {
      for (const scheme of contract.pixelPolicy.schemes) {
        const context = await browser.newContext({
          viewport: { width, height: width === 390 ? 844 : 860 },
          deviceScaleFactor: width === 390 ? 2 : 1,
          isMobile: width === 390,
          hasTouch: width === 390,
          colorScheme: scheme,
          reducedMotion: 'reduce',
          locale: 'en-US',
          timezoneId: 'UTC',
        });
        await context.clock.install({ time: new Date('2026-10-19T12:00:00.000Z') });
        const page = await context.newPage();
        const pageErrors = [];
        await context.route('**/*', (route) =>
          new URL(route.request().url()).origin === server.origin
            ? route.continue()
            : (pageErrors.push(`blocked nonlocal request ${route.request().url()}`), route.abort()),
        );
        page.on('response', (response) => {
          if (response.status() >= 400)
            pageErrors.push(`HTTP ${response.status()} ${response.url()}`);
        });
        page.on('pageerror', (error) => pageErrors.push(error.message));
        const url = `${server.origin}/iframe.html?id=${encodeURIComponent(storyId)}&viewMode=story&globals=theme%3A${scheme}`;
        try {
          await page.goto(url, { waitUntil: 'networkidle', timeout: 30000 });
          await page.waitForSelector('#storybook-root > *', { timeout: 15000 });
          await page.evaluate(async () => {
            await document.fonts.ready;
            await document.fonts.load('400 16px "Instrument Sans"');
            await document.fonts.load('400 12px "JetBrains Mono"');
            await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          });
          const runtimeChecks = await page.evaluate(() => ({
            sans: document.fonts.check('400 16px "Instrument Sans"'),
            mono: document.fonts.check('400 12px "JetBrains Mono"'),
            viewportWidth: window.innerWidth,
            documentWidth: document.documentElement.scrollWidth,
            focus: document.activeElement instanceof HTMLElement
              ? {
                  tag: document.activeElement.tagName,
                  label: document.activeElement.getAttribute('aria-label'),
                  sessionRow: document.activeElement.dataset.sessionRow,
                }
              : null,
          }));
          if (!runtimeChecks.sans || !runtimeChecks.mono) pageErrors.push('required production font failed to load');
          if (runtimeChecks.documentWidth > runtimeChecks.viewportWidth + 1) {
            pageErrors.push(`horizontal document overflow ${runtimeChecks.documentWidth}px > ${runtimeChecks.viewportWidth}px`);
          }
          if (storyId.endsWith('--keyboard-focus') && runtimeChecks.focus?.sessionRow !== 'list') {
            pageErrors.push(`keyboard-focus story did not focus the production session row: ${JSON.stringify(runtimeChecks.focus)}`);
          }
          const file = path.join(output, storyId, `${width}-${scheme}.png`);
          fs.mkdirSync(path.dirname(file), { recursive: true });
          const bytes = await page.screenshot({ path: file, fullPage: true, animations: 'disabled' });
          const key = `${storyId}|${width}|${scheme}`;
          const result = { key, storyId, width, scheme, candidate: path.relative(here, file), sha256: sha256(bytes) };
          if (pageErrors.length) {
            failures.push(`${key}: ${pageErrors.join('; ')}`);
            result.pageErrors = pageErrors;
          }

          const approval = referenceMap.approved?.[key];
          if (!approval || approval.decision !== 'accepted') {
            pending.push(key);
            result.reference = 'pending';
          } else {
            const reference = path.resolve(here, approval.path);
            if (!fs.existsSync(reference)) {
              failures.push(`${key}: approved reference file is missing`);
              result.reference = 'missing';
            } else {
              const referenceBytes = fs.readFileSync(reference);
              if (
                !reference.startsWith(path.resolve(here, 'baselines/storybook') + path.sep) ||
                approval.storyId !== storyId ||
                approval.width !== width ||
                approval.scheme !== scheme ||
                approval.threshold !== contract.pixelPolicy.threshold ||
                approval.maxRatio !== contract.pixelPolicy.maxRatio ||
                approval.interactionId != null
              ) {
                failures.push(`${key}: reference binding does not match the fixed story capture contract`);
                result.reference = 'invalid-binding';
              } else if (sha256(referenceBytes) !== approval.sha256) {
                failures.push(`${key}: approved reference digest changed`);
                result.reference = 'digest-mismatch';
              } else {
                const diff = compare(PNG.sync.read(referenceBytes), PNG.sync.read(bytes), approval.threshold);
                result.ratio = diff.ratio;
                if (diff.size || diff.ratio > approval.maxRatio) {
                  failures.push(`${key}: ${diff.size ? 'image dimensions changed' : `pixel mismatch ${(100 * diff.ratio).toFixed(3)}%`}`);
                  const diffPath = path.join(output, storyId, `${width}-${scheme}-diff.png`);
                  fs.writeFileSync(diffPath, PNG.sync.write(diff.diff));
                  result.diff = path.relative(here, diffPath);
                }
                result.reference = 'accepted';
              }
            }
          }
          results.push(result);
        } catch (error) {
          failures.push(`${storyId}|${width}|${scheme}: ${error instanceof Error ? error.message : String(error)}`);
        } finally {
          await context.close();
        }
      }
    }
  }
  for (const interaction of contract.interactionCaptures ?? []) {
    for (const width of interaction.widths) {
      for (const scheme of interaction.schemes) {
        const context = await browser.newContext({
          viewport: { width, height: width === 390 ? 844 : 860 },
          deviceScaleFactor: width === 390 ? 2 : 1,
          isMobile: width === 390,
          hasTouch: width === 390,
          colorScheme: scheme,
          reducedMotion: 'reduce',
          locale: 'en-US',
          timezoneId: 'UTC',
        });
        await context.clock.install({ time: new Date('2026-10-19T12:00:00.000Z') });
        const page = await context.newPage();
        const pageErrors = [];
        await context.route('**/*', (route) =>
          new URL(route.request().url()).origin === server.origin
            ? route.continue()
            : (pageErrors.push(`blocked nonlocal request ${route.request().url()}`), route.abort()),
        );
        page.on('response', (response) => {
          if (response.status() >= 400) pageErrors.push(`HTTP ${response.status()} ${response.url()}`);
        });
        page.on('pageerror', (error) => pageErrors.push(error.message));
        const key = `${interaction.storyId}|${width}|${scheme}|${interaction.id}`;
        try {
          await page.goto(
            `${server.origin}/iframe.html?id=${encodeURIComponent(interaction.storyId)}&viewMode=story&globals=theme%3A${scheme}`,
            { waitUntil: 'networkidle', timeout: 30000 },
          );
          await page.waitForSelector('#storybook-root > *', { timeout: 15000 });
          for (const action of interaction.actions) {
            const target = page.locator(action.selector).first();
            await target.waitFor({ state: 'visible', timeout: 10000 });
            if (action.kind === 'click') await target.click();
            else if (action.kind === 'hover') await target.hover();
            else if (action.kind === 'focus') await target.focus();
            else throw new Error(`unknown action kind ${action.kind}`);
            if (action.waitMs) await page.waitForTimeout(action.waitMs);
            await verifyAssertions(page, action.assertionsAfter, pageErrors, key);
          }
          await verifyAssertions(page, interaction.assertions, pageErrors, key);
          await page.evaluate(async () => {
            await document.fonts.ready;
            await document.fonts.load('400 16px "Instrument Sans"');
            await document.fonts.load('400 12px "JetBrains Mono"');
            await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          });
          const geometry = await page.evaluate(() => ({
            sans: document.fonts.check('400 16px "Instrument Sans"'),
            mono: document.fonts.check('400 12px "JetBrains Mono"'),
            viewportWidth: window.innerWidth,
            documentWidth: document.documentElement.scrollWidth,
          }));
          if (!geometry.sans || !geometry.mono) pageErrors.push('required production font failed to load');
          if (geometry.documentWidth > geometry.viewportWidth + 1) {
            pageErrors.push(`horizontal document overflow ${geometry.documentWidth}px > ${geometry.viewportWidth}px`);
          }
          const file = path.join(output, interaction.storyId, `state-${interaction.id}-${width}-${scheme}.png`);
          fs.mkdirSync(path.dirname(file), { recursive: true });
          const bytes = await page.screenshot({ path: file, fullPage: true, animations: 'disabled' });
          const result = {
            key,
            storyId: interaction.storyId,
            interactionId: interaction.id,
            width,
            scheme,
            candidate: path.relative(here, file),
            sha256: sha256(bytes),
            actions: interaction.actions,
            assertions: interaction.assertions,
            postCaptureActions: interaction.postCaptureActions ?? [],
            postCaptureAssertions: interaction.postCaptureAssertions ?? [],
          };
          for (const action of interaction.postCaptureActions ?? []) {
            const target = page.locator(action.selector).first();
            await target.waitFor({ state: 'visible', timeout: 10000 });
            if (action.kind === 'click') await target.click();
            else if (action.kind === 'hover') await target.hover();
            else if (action.kind === 'focus') await target.focus();
            else throw new Error(`unknown post-capture action kind ${action.kind}`);
          }
          await verifyAssertions(page, interaction.postCaptureAssertions, pageErrors, key);
          if (pageErrors.length) {
            failures.push(`${key}: ${pageErrors.join('; ')}`);
            result.pageErrors = pageErrors;
          }
          const approval = referenceMap.approved?.[key];
          if (!approval || approval.decision !== 'accepted') {
            pending.push(key);
            result.reference = 'pending';
          } else {
            const reference = path.resolve(here, approval.path);
            if (!fs.existsSync(reference)) {
              failures.push(`${key}: approved reference file is missing`);
              result.reference = 'missing';
            } else {
              const referenceBytes = fs.readFileSync(reference);
              if (
                !reference.startsWith(path.resolve(here, 'baselines/storybook') + path.sep) ||
                approval.storyId !== interaction.storyId ||
                approval.interactionId !== interaction.id ||
                approval.width !== width ||
                approval.scheme !== scheme ||
                approval.threshold !== contract.pixelPolicy.threshold ||
                approval.maxRatio !== contract.pixelPolicy.maxRatio
              ) {
                failures.push(`${key}: reference binding does not match the fixed interaction capture contract`);
                result.reference = 'invalid-binding';
              } else if (sha256(referenceBytes) !== approval.sha256) {
                failures.push(`${key}: approved reference digest changed`);
                result.reference = 'digest-mismatch';
              } else {
                const diff = compare(PNG.sync.read(referenceBytes), PNG.sync.read(bytes), approval.threshold);
                result.ratio = diff.ratio;
                if (diff.size || diff.ratio > approval.maxRatio) {
                  failures.push(`${key}: ${diff.size ? 'image dimensions changed' : `pixel mismatch ${(100 * diff.ratio).toFixed(3)}%`}`);
                  const diffPath = path.join(output, interaction.storyId, `state-${interaction.id}-${width}-${scheme}-diff.png`);
                  fs.writeFileSync(diffPath, PNG.sync.write(diff.diff));
                  result.diff = path.relative(here, diffPath);
                }
                result.reference = 'accepted';
              }
            }
          }
          results.push(result);
        } catch (error) {
          failures.push(`${key}: ${error instanceof Error ? error.message : String(error)}`);
        } finally {
          await context.close();
        }
      }
    }
  }
} finally {
  if (browser) await browser.close();
  if (server) await server.close();
}

const provenanceFiles = [
  'ui/stories/ProductionSurfaces.stories.tsx',
  'tests/ui/storybook-contract.json',
  'ui/package-lock.json',
  'crates/semon-sessions/src/viewer.css',
  'crates/semon-sessions/src/select.css',
  'crates/semon-sessions/src/fonts/instrument-sans/latin.woff2',
  'crates/semon-sessions/src/fonts/instrument-sans/latin-ext.woff2',
  'crates/semon-sessions/src/fonts/jetbrains-mono/latin.woff2',
  'crates/semon-sessions/src/fonts/jetbrains-mono/latin-ext.woff2',
  'crates/semon-sessions/src/mark.svg',
  'tests/ui/fixture-data/semon-sample.json',
  'tests/ui/fixture-data/overhaul.json',
];
const provenance = Object.fromEntries(
  provenanceFiles.map((relative) => {
    const file = path.resolve(here, '../..', relative);
    return [relative, fs.existsSync(file) ? sha256(fs.readFileSync(file)) : 'missing'];
  }),
);

const report = {
  candidateOnly: true,
  approvedReferencesWritten: false,
  captureDimensions: [...new Set([
    ...contract.pixelPolicy.dimensions,
    ...(contract.interactionCaptures ?? []).flatMap((item) => item.widths),
  ])].sort((a, b) => a - b),
  colorSchemes: contract.pixelPolicy.schemes,
  interactionCaptureCount: contract.interactionCaptures?.length ?? 0,
  fixedClock: '2026-10-19T12:00:00.000Z',
  locale: 'en-US',
  timezone: 'UTC',
  browserVersion,
  sourceRevision: process.env.GITHUB_SHA ?? null,
  provenance,
  storyCount: storyIds.length,
  results,
  pending,
  failures,
};
fs.writeFileSync(path.join(output, 'capture-report.json'), JSON.stringify(report, null, 2) + '\n');
if (pending.length || failures.length) {
  console.error(
    `Storybook captures: ${results.length} candidates, ${pending.length} pending/unapproved references, ${failures.length} capture/comparison failures.`,
  );
  process.exitCode = 1;
} else {
  console.log(`Storybook captures: ${results.length} candidate images matched all approved references.`);
}

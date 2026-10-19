import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { PNG } from 'pngjs';
import { launch } from './lib.mjs';
import { compare } from './pixel-diff.mjs';
import { createStorybookServer } from './storybook-server.mjs';
import { makeCapturePlan } from './storybook-plan.mjs';
import { verifyAssertions } from './storybook-assertions.mjs';
import { referenceErrors, referenceComparisonErrors, captureIdentityDigest } from './storybook-references.mjs';
import { withCapturePaint, captureStableFrame } from './capture-paint.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const readJson = (file) => JSON.parse(fs.readFileSync(path.resolve(here, file), 'utf8'));
const contract = readJson('storybook-contract.json');
const inventory = readJson(contract.stateBindings);
const referenceMap = readJson(contract.referenceMap);
const storyDir = path.resolve(here, 'storybook-static');
const storyIndex = readJson(contract.storyIndex);
const storyIds = Object.entries(storyIndex.entries ?? {})
  .filter(([id, entry]) => entry.type === 'story' && (contract.ownedStoryPrefixes ?? [contract.ownedStoryPrefix]).some((prefix) => id.startsWith(prefix)))
  .map(([id]) => id).sort();
const plan = makeCapturePlan(contract, inventory, storyIds);
const invalidReferences = referenceErrors(referenceMap, plan);
if (invalidReferences.length) throw new Error(invalidReferences.join('\n'));
const pending = [], failures = [], results = [];
const output = path.resolve(process.env.SEMON_UI_OUT ?? path.join(here, 'out'), 'storybook/candidates');
fs.mkdirSync(output, { recursive: true });
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const provenanceFiles = [
  'ui/stories/ProductionSurfaces.stories.tsx', 'ui/stories/fixtures.css', 'ui/tests/fixtures/design-gallery.tsx',
  'ui/.storybook/preview.tsx', 'ui/.storybook/preview.css', 'ui/.storybook/main.mjs',
  'tests/ui/storybook-contract.json', 'tests/ui/storybook-state-bindings.json',
  'tests/ui/storybook-plan.mjs', 'tests/ui/storybook-capture.mjs',
  'tests/ui/storybook-assertions.mjs',
  'tests/ui/storybook-references.mjs', 'tests/ui/storybook-reference-map.schema.json',
  'tests/ui/storybook-legacy.mjs', 'tests/ui/storybook-gates.json',
  'ui/stories/LegacyRoutes.stories.tsx', 'ui/stories/LegacyRoutes.data.ts', 'ui/stories/LegacyRoutes.analytics.ts',
  'ui/stories/LegacyGallery.stories.tsx', 'tests/ui/fixture-data/overhaul.json', 'tests/ui/fixture.mjs', 'tests/ui/gallery.css', 'tests/ui/capture-paint.mjs',
  'ui/package-lock.json', 'tests/ui/package-lock.json',
  'crates/semon-sessions/src/viewer.css', 'crates/semon-sessions/src/select.css',
  'crates/semon-sessions/src/shell.css',
  'crates/semon-sessions/src/fonts/instrument-sans/latin.woff2',
  'crates/semon-sessions/src/fonts/instrument-sans/latin-ext.woff2',
  'crates/semon-sessions/src/fonts/jetbrains-mono/latin.woff2',
  'crates/semon-sessions/src/fonts/jetbrains-mono/latin-ext.woff2',
  'crates/semon-sessions/src/mark.svg',
  'assets/harnesses/codex-black.svg', 'assets/harnesses/codex.svg',
];
// Imported production owners are part of capture provenance, including changes
// that leave the story fixture text unchanged. This traversal runs only in CI.
function sourceFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(directory, entry.name);
    return entry.isDirectory() ? sourceFiles(file) : [file];
  });
}
for (const file of sourceFiles(path.resolve(here, '../../ui/src')))
  provenanceFiles.push(path.relative(path.resolve(here, '../..'), file));
const provenance = Object.fromEntries(provenanceFiles.sort().map((relative) => [
  relative, sha256(fs.readFileSync(path.resolve(here, '../..', relative))),
]));
const sourceDigest = sha256(JSON.stringify(provenance));

async function perform(page, actions) {
  for (const action of actions) {
    const target = page.locator(action.selector);
    await target.waitFor({ state: 'visible', timeout: 10000 });
    if (action.kind === 'click') await target.click();
    else if (action.kind === 'tap') await target.tap();
    else if (action.kind === 'hover') await target.hover();
    else if (action.kind === 'focus') await target.focus();
    else if (action.kind === 'press') await target.press(action.key);
    else if (action.kind === 'owner-unmount')
      await target.evaluate((owner) => owner.dispatchEvent(new Event('storybook:unmount')));
    else if (action.kind === 'scroll-top') await target.evaluate((node) => { node.scrollTop = 0; });
    else throw new Error(`unknown action kind ${action.kind}`);
    if (action.waitMs) await page.waitForTimeout(action.waitMs);
    await verifyAssertions(page, action.assertionsAfter ?? []);
  }
}

function compareReference(item, bytes, result) {
  const approval = referenceMap.approved?.[item.key];
  if (!approval || approval.decision !== 'accepted') {
    pending.push(item.key);
    result.reference = 'pending';
    return;
  }
  const reference = path.resolve(here, approval.path);
  if (!reference.startsWith(path.resolve(here, 'baselines/storybook') + path.sep))
    throw new Error('Reference path is outside the approved baseline directory');
  const referenceBytes = fs.readFileSync(reference);
  const bindingErrors = referenceComparisonErrors(approval, item, referenceBytes, {
    sourceDigest, sourceRevision: process.env.GITHUB_SHA ?? null,
  });
  if (bindingErrors.length) throw new Error(bindingErrors.join('; '));
  result.referenceProvenance = {
    sourceDigest: approval.sourceDigest, reviewedRevision: approval.reviewedRevision,
    reviewedBy: approval.reviewedBy, reviewedAt: approval.reviewedAt,
    imageDigest: approval.sha256, captureDigest: approval.captureDigest,
  };
  const diff = compare(PNG.sync.read(referenceBytes), PNG.sync.read(bytes), approval.threshold);
  result.ratio = diff.ratio;
  result.reference = 'accepted';
  if (diff.size || diff.ratio > approval.maxRatio) {
    const diffPath = path.resolve(here, result.candidate.replace(/\.png$/, '-diff.png'));
    fs.writeFileSync(diffPath, PNG.sync.write(diff.diff));
    result.diff = path.relative(here, diffPath);
    throw new Error(diff.size ? 'image dimensions changed' : `pixel mismatch ${(100 * diff.ratio).toFixed(3)}%`);
  }
}

let browser, server, browserVersion = null;
try {
  server = await createStorybookServer(storyDir);
  browser = await launch({ baselineRaster: true });
  browserVersion = browser.version();
  const captureOne = async (item) => {
    const { storyId, width, scheme } = item;
    const viewport = item.viewport ?? { width, height: width <= 390 ? 844 : 860, deviceScaleFactor: width <= 390 ? 2 : 1, isMobile: width <= 390, hasTouch: width < 1000 };
    const context = await browser.newContext({
      viewport: { width, height: viewport.height }, deviceScaleFactor: viewport.deviceScaleFactor,
      isMobile: viewport.isMobile, hasTouch: viewport.hasTouch,
      colorScheme: scheme, reducedMotion: item.reducedMotion ?? 'reduce', locale: 'en-US', timezoneId: 'UTC',
    });
    const pageErrors = [];
    const result = { ...item, candidate: null, sourceDigest, captureDigest: captureIdentityDigest(item) };
    try {
      await context.clock.install({ time: new Date(item.clock ?? '2026-10-19T12:00:00.000Z') });
      const page = await context.newPage();
      await context.route('**/*', (route) =>
        new URL(route.request().url()).origin === server.origin ? route.continue() :
          (pageErrors.push(`blocked nonlocal request ${route.request().url()}`), route.abort()));
      page.on('response', (response) => { if (response.status() >= 400) pageErrors.push(`HTTP ${response.status()} ${response.url()}`); });
      page.on('pageerror', (error) => pageErrors.push(error.message));
      const args = Object.entries(item.args ?? {}).map(([key, value]) => `${key}:${value}`).join(';');
      await page.goto(`${server.origin}/iframe.html?id=${encodeURIComponent(storyId)}&viewMode=story&globals=theme%3A${scheme}${args ? '&args=' + encodeURIComponent(args) : ''}`, { waitUntil: 'networkidle', timeout: 30000 });
      await page.waitForSelector('#storybook-root > *', { timeout: 15000 });
      await perform(page, item.actions);
      await verifyAssertions(page, item.assertions);
      if (storyId.endsWith('--keyboard-focus'))
        await verifyAssertions(page, [{ kind: 'focused', selector: '[data-session-row="list"]' }]);
      await page.evaluate(async () => {
        await document.fonts.ready;
        await document.fonts.load('400 16px "Instrument Sans"');
        await document.fonts.load('400 12px "JetBrains Mono"');
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      });
      const geometry = await page.evaluate(() => ({
        sans: document.fonts.check('400 16px "Instrument Sans"'),
        mono: document.fonts.check('400 12px "JetBrains Mono"'),
        viewportWidth: innerWidth, documentWidth: document.documentElement.scrollWidth,
      }));
      if (!geometry.sans || !geometry.mono) throw new Error('required production font failed to load');
      if (geometry.documentWidth > geometry.viewportWidth + 1)
        throw new Error(`horizontal document overflow ${geometry.documentWidth}px > ${geometry.viewportWidth}px`);
      if (item.capture?.top) await page.evaluate(() => { window.scrollTo(0, 0); const main = document.querySelector('#main'); if (main) main.scrollTop = 0; });
      if (item.capture?.viewportHeight === 'fit-transcript-and-dock-plus-80') {
        const target = page.locator(item.capture.selector);
        const box = await target.boundingBox();
        if (!box || await target.count() !== 1) throw new Error('Exact transcript region must have one measurable owner');
        const dock = page.locator('.session-dock');
        const dockHeight = await dock.count() ? (await dock.boundingBox())?.height ?? 0 : 0;
        await page.setViewportSize({ width, height: Math.ceil(Math.max(viewport.height, box.y + box.height + dockHeight + 80)) });
        const after = await target.boundingBox();
        if (!after || Math.abs(box.width - after.width) > 0.5 || Math.abs(box.height - after.height) > 0.5) throw new Error('Transcript geometry changed while fitting capture viewport');
      }
      if (item.expectedState?.includes('sessions-filter-open')) await verifyAssertions(page, [{ kind: 'visible', selector: '.filters-sheet .vb', scrollTop: 0 }, { kind: 'focused', selector: '.filters-sheet[open] .vclose' }]);
      const file = path.join(output, storyId, `${item.legacyKey ? sha256(item.key).slice(0, 20) + '-' : item.interactionId ? `state-${item.interactionId}-` : ''}${width}-${scheme}.png`);
      fs.mkdirSync(path.dirname(file), { recursive: true });
      const options = item.capture ? { fullPage: item.capture.fullPage, animations: item.capture.animations, ...(item.capture.caret ? { caret: item.capture.caret } : {}) } : { fullPage: true, animations: 'disabled' };
      const shot = async () => {
        if (item.capture?.selector) {
          const target = page.locator(item.capture.selector);
          if (await target.count() !== 1) throw new Error('Capture region requires exactly one owner');
          if (item.capture.viewportHeight === 'fit-transcript-and-dock-plus-80') {
            const clip = await target.boundingBox();
            if (!clip) throw new Error('Missing transcript capture rectangle');
            return page.screenshot({ clip, animations: options.animations, caret: options.caret });
          }
          return target.screenshot({ animations: options.animations, caret: options.caret });
        }
        return item.capture?.stablePaint ? captureStableFrame(page, options) : page.screenshot(options);
      };
      const bytes = item.capture?.stablePaint ? await withCapturePaint(page, shot) : await shot();
      fs.writeFileSync(file, bytes);
      result.actualViewport = page.viewportSize();
      result.captureGeometry = item.capture?.selector ? await page.locator(item.capture.selector).boundingBox() : null;
      result.candidate = path.relative(here, file);
      result.sha256 = sha256(bytes);
      await perform(page, item.postCaptureActions);
      await verifyAssertions(page, item.postCaptureAssertions);
      if (pageErrors.length) throw new Error(pageErrors.join('; '));
      result.rendered = true;
      try { compareReference(item, bytes, result); }
      catch (error) {
        result.comparisonError = error instanceof Error ? error.message : String(error);
        failures.push(`${item.key}: ${result.comparisonError}`);
      }
    } catch (error) {
      result.error = error instanceof Error ? error.message : String(error);
      failures.push(`${item.key}: ${result.error}`);
    } finally {
      results.push(result);
      await context.close();
    }
  };
  // The complete inventory contains hundreds of isolated pages. Three bounded
  // CI workers keep the full plan within the existing job allowance; each case
  // still owns a fresh browser context, clock, fixture, theme and output path.
  let next = 0;
  await Promise.all(Array.from({ length: 3 }, async () => {
    while (next < plan.length) {
      const item = plan[next++];
      try { await captureOne(item); }
      catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        failures.push(`${item.key}: context setup/cleanup failed: ${message}`);
        const row = results.find((entry) => entry.key === item.key);
        if (row) { row.rendered = false; row.error = message; }
        else results.push({ ...item, rendered: false, candidate: null, error: message });
      }
    }
  }));
} finally {
  if (browser) await browser.close();
  if (server) await server.close();
}

const expectedKeys = new Set(plan.map((item) => item.key));
for (const key of Object.keys(referenceMap.approved ?? {}))
  if (!expectedKeys.has(key)) failures.push(`unknown/orphaned approved reference ${key}`);
const completed = new Set(results.filter((row) => row.candidate && row.rendered && !row.error).map((row) => row.key));
for (const key of expectedKeys) if (!completed.has(key)) failures.push(`missing successful capture ${key}`);
const report = {
  candidateOnly: true, approvedReferencesWritten: false,
  captureDimensions: [...new Set(plan.map((item) => item.width))].sort((a, b) => a - b),
  colorSchemes: contract.pixelPolicy.schemes, interactionCaptureCount: contract.interactionCaptures.length,
  fixedClock: '2026-10-19T12:00:00.000Z', locale: 'en-US', timezone: 'UTC', browserVersion,
  fixedClocks: [...new Set(plan.map((item) => item.clock ?? '2026-10-19T12:00:00.000Z'))],
  contextWorkers: 3,
  sourceRevision: process.env.GITHUB_SHA ?? null, provenance, sourceDigest,
  storyCount: storyIds.length, expectedKeys: [...expectedKeys], results, pending, failures,
};
fs.writeFileSync(path.join(output, 'capture-report.json'), JSON.stringify(report, null, 2) + '\n');
console.log(`Storybook captures: ${completed.size}/${expectedKeys.size} rendered, ${pending.length} pending references, ${failures.length} failures.`);
if (pending.length || failures.length) process.exitCode = 1;

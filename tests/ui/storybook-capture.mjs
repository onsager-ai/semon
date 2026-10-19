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

const here = path.dirname(fileURLToPath(import.meta.url));
const readJson = (file) => JSON.parse(fs.readFileSync(path.resolve(here, file), 'utf8'));
const contract = readJson('storybook-contract.json');
const inventory = readJson(contract.stateBindings);
const referenceMap = readJson(contract.referenceMap);
const storyDir = path.resolve(here, 'storybook-static');
const storyIndex = readJson(contract.storyIndex);
const storyIds = Object.entries(storyIndex.entries ?? {})
  .filter(([id, entry]) => entry.type === 'story' && id.startsWith(contract.ownedStoryPrefix))
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
    const diffPath = path.join(path.dirname(path.resolve(here, result.candidate)), `${item.width}-${item.scheme}${item.interactionId ? `-${item.interactionId}` : ''}-diff.png`);
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
  for (const item of plan) {
    const { storyId, width, scheme } = item;
    const context = await browser.newContext({
      viewport: { width, height: width <= 390 ? 844 : 860 },
      deviceScaleFactor: width <= 390 ? 2 : 1,
      isMobile: width <= 390, hasTouch: width < 1000,
      colorScheme: scheme, reducedMotion: 'reduce', locale: 'en-US', timezoneId: 'UTC',
    });
    await context.clock.install({ time: new Date('2026-10-19T12:00:00.000Z') });
    const page = await context.newPage();
    const pageErrors = [];
    const result = { ...item, candidate: null, sourceDigest, captureDigest: captureIdentityDigest(item) };
    try {
      await context.route('**/*', (route) =>
        new URL(route.request().url()).origin === server.origin ? route.continue() :
          (pageErrors.push(`blocked nonlocal request ${route.request().url()}`), route.abort()));
      page.on('response', (response) => { if (response.status() >= 400) pageErrors.push(`HTTP ${response.status()} ${response.url()}`); });
      page.on('pageerror', (error) => pageErrors.push(error.message));
      await page.goto(`${server.origin}/iframe.html?id=${encodeURIComponent(storyId)}&viewMode=story&globals=theme%3A${scheme}`, { waitUntil: 'networkidle', timeout: 30000 });
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
      const file = path.join(output, storyId, `${item.interactionId ? `state-${item.interactionId}-` : ''}${width}-${scheme}.png`);
      fs.mkdirSync(path.dirname(file), { recursive: true });
      const bytes = await page.screenshot({ path: file, fullPage: true, animations: 'disabled' });
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
  }
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
  sourceRevision: process.env.GITHUB_SHA ?? null, provenance, sourceDigest,
  storyCount: storyIds.length, expectedKeys: [...expectedKeys], results, pending, failures,
};
fs.writeFileSync(path.join(output, 'capture-report.json'), JSON.stringify(report, null, 2) + '\n');
console.log(`Storybook captures: ${completed.size}/${expectedKeys.size} rendered, ${pending.length} pending references, ${failures.length} failures.`);
if (pending.length || failures.length) process.exitCode = 1;

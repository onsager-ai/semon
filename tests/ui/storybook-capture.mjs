import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { PNG } from 'pngjs';
import { launch } from './lib.mjs';
import { compare } from './pixel-diff.mjs';

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

const contentTypes = {
  '.css': 'text/css; charset=utf-8',
  '.html': 'text/html; charset=utf-8',
  '.ico': 'image/x-icon',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.map': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.woff2': 'font/woff2',
};
const assets = new Map();
function collectAssets(directory, prefix = '') {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    const key = path.posix.join(prefix, entry.name);
    if (entry.isDirectory()) collectAssets(file, key);
    else if (entry.isFile()) assets.set('/' + key.replaceAll(path.sep, '/'), file);
  }
}
collectAssets(storyDir);

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function safePath(urlPath) {
  const pathname = decodeURIComponent(new URL(urlPath, 'http://storybook.local').pathname);
  const file = path.resolve(storyDir, '.' + pathname);
  return file === storyDir || file.startsWith(storyDir + path.sep) ? file : null;
}

let browser;
try {
  browser = await launch({ baselineRaster: true });
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
        });
        const page = await context.newPage();
        const pageErrors = [];
        await page.route('**/*', async (route) => {
          const url = new URL(route.request().url());
          if (url.origin !== 'http://storybook.test') {
            pageErrors.push(`blocked nonlocal request ${route.request().url()}`);
            return route.abort();
          }
          const file = safePath(url.pathname);
          const asset = file ? assets.get(url.pathname) : null;
          if (!asset || !fs.existsSync(asset)) {
            pageErrors.push(`missing Storybook asset ${url.pathname}`);
            return route.fulfill({ status: 404, body: 'Not found' });
          }
          return route.fulfill({
            status: 200,
            contentType: contentTypes[path.extname(asset)] ?? 'application/octet-stream',
            body: fs.readFileSync(asset),
            headers: {
              'cache-control': 'no-store',
              'content-security-policy': "default-src 'self'; img-src 'self' data:; font-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'",
              'x-content-type-options': 'nosniff',
            },
          });
        });
        page.on('response', (response) => {
          if (response.status() >= 400)
            pageErrors.push(`HTTP ${response.status()} ${response.url()}`);
        });
        page.on('pageerror', (error) => pageErrors.push(error.message));
        const url = `http://storybook.test/iframe.html?id=${encodeURIComponent(storyId)}&viewMode=story&globals=theme%3A${scheme}`;
        try {
          await page.goto(url, { waitUntil: 'networkidle', timeout: 30000 });
          await page.waitForSelector('#storybook-root > *', { timeout: 15000 });
          await page.evaluate(async () => {
            await document.fonts.ready;
            await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
          });
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
                approval.maxRatio !== contract.pixelPolicy.maxRatio
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
} finally {
  if (browser) await browser.close();
}

const report = {
  candidateOnly: true,
  approvedReferencesWritten: false,
  captureDimensions: contract.pixelPolicy.dimensions,
  colorSchemes: contract.pixelPolicy.schemes,
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

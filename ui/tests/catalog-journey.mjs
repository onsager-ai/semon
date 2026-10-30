// Read only a local fixture authorization file; evidence excludes its credential.
// node ui/tests/catalog-journey.mjs fixture.json evidence.json
// fixture: {base,token?,source_key,catalog_key,source_revision?,harness?}
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
const fixture = JSON.parse(
  await readFile(process.argv[2] ?? process.env.SEMON_CATALOG_SERVED_MANIFEST, 'utf8'),
);
const MAX_NETWORK_EVENTS = 200;
fixture.catalog_key ??= fixture.sid;
const evidencePath = process.argv[3] ?? fixture.evidence_path;
await writeFile(
  evidencePath + '.ready',
  JSON.stringify({ pid: process.pid, phase: fixture.phase ?? null }),
);
const browser = await chromium.launch();
const errors = [],
  requests = [],
  responses = [];
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  const page = await context.newPage(),
    cdp = await context.newCDPSession(page);
  await cdp.send('Performance.enable');
  const fieldTexts = [];
  const captures = new Set(),
    fieldCaptures = new Set();
  let bundle = null;
  page.on('pageerror', (error) => {
    if (errors.length < MAX_NETWORK_EVENTS) errors.push(error.message);
  });
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/') && requests.length < MAX_NETWORK_EVENTS) {
      url.searchParams.delete('t');
      requests.push(url.pathname + url.search);
    }
  });
  page.on('response', (response) => {
    const capture = (async () => {
      const url = new URL(response.url());
      if (url.pathname === '/viewer.js')
        bundle = createHash('sha256')
          .update(await response.body())
          .digest('hex');
      if (!url.pathname.startsWith('/api/')) return;
      const started = response.request().timing();
      if ((response.headers()['content-type'] ?? '').includes('text/event-stream')) {
        if (responses.length < MAX_NETWORK_EVENTS)
          responses.push({
            path: url.pathname,
            status: response.status(),
            bytes: null,
            streamed: true,
            serverResponseMs: started.responseStart - started.requestStart,
          });
        return;
      }
      let bytes = null,
        body = null,
        unavailable = null,
        timer;
      try {
        body = await Promise.race([
          response.body(),
          new Promise((_, reject) => {
            timer = setTimeout(() => reject(Error('Response capture deadline')), 6000);
          }),
        ]);
        bytes = body.length;
      } catch (error) {
        unavailable = error.message;
      } finally {
        clearTimeout(timer);
      }
      if (url.pathname === '/api/session-entry' && response.status() === 200) {
        if (!body) throw Error('Native field response bytes are unavailable');
        const field = JSON.parse(body.toString('utf8'));
        fieldTexts[field.field.chunk] = field.text;
      }
      if (responses.length < MAX_NETWORK_EVENTS)
        responses.push({
          path: url.pathname,
          status: response.status(),
          bytes,
          ...(unavailable ? { body_unavailable: unavailable } : {}),
          serverResponseMs: started.responseStart - started.requestStart,
        });
    })().catch((error) => {
      if (errors.length < MAX_NETWORK_EVENTS)
        errors.push('Response capture failed: ' + error.message);
    });
    captures.add(capture);
    if (new URL(response.url()).pathname === '/api/session-entry' && response.status() === 200)
      fieldCaptures.add(capture);
    void capture.finally(() => {
      captures.delete(capture);
      fieldCaptures.delete(capture);
    });
  });
  async function sample() {
    const metrics = await cdp.send('Performance.getMetrics');
    return {
      domNodes: await page.locator('*').count(),
      entries: await page.locator('[data-entry-key]').count(),
      rows: await page.locator('[data-session-row="list"]').count(),
      ...Object.fromEntries(
        metrics.metrics
          .filter((item) =>
            [
              'TaskDuration',
              'ScriptDuration',
              'LayoutDuration',
              'JSHeapUsedSize',
              'Nodes',
            ].includes(item.name),
          )
          .map((item) => [item.name, item.value]),
      ),
    };
  }
  const before = await sample(),
    url = new URL('/sessions', fixture.base);
  url.searchParams.set('machine', fixture.source_key);
  if (fixture.token) url.searchParams.set('t', fixture.token);
  let started = performance.now();
  try {
    await page.goto(url.href);
  } catch {
    throw Error('Fixture navigation failed');
  }
  await page.locator('[data-session-row="list"]').first().waitFor({ timeout: 60000 });
  // Navigation to the first visible catalog row includes fixture import/readiness;
  // it is not a browser Paint Timing entry or a production latency sample.
  const firstVisibleCatalogRowMs = performance.now() - started,
    cold = await sample();
  const paintTiming = await page.evaluate(async () => {
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const entries = performance.getEntriesByType('paint'),
      expected = ['first-paint', 'first-contentful-paint'],
      supported = PerformanceObserver.supportedEntryTypes.includes('paint');
    return {
      timeOrigin: performance.timeOrigin,
      navigationType: performance.getEntriesByType('navigation')[0]?.type ?? null,
      entries: entries.map(({ name, startTime }) => ({ name, startTimeMs: startTime })),
      unavailable: expected
        .filter((name) => !entries.some((entry) => entry.name === name))
        .map((name) => ({
          name,
          reason: supported
            ? 'No entry reported after two animation frames following the first visible catalog row'
            : 'Paint Timing entries are unsupported in this browser',
        })),
    };
  });
  if (fixture.harness) {
    started = performance.now();
    await page.getByRole('button', { name: 'Filter', exact: true }).click();
    const sheet = page.getByRole('dialog', { name: 'Filter', exact: true }),
      harness = sheet.locator('.sh-select[data-label="Harness"]');
    await harness.locator('.sh-select-trigger').click();
    await harness.locator(`[role="option"][data-value="${fixture.harness}"]`).click();
    // The shared sheet commits its selected facets together when Done closes it.
    const [response] = await Promise.all([
      page.waitForResponse((response) => {
        const url = new URL(response.url());
        return (
          url.pathname === '/api/sessions' &&
          url.searchParams.get('harness') === fixture.harness &&
          url.searchParams.get('machine') === fixture.source_key
        );
      }),
      sheet.getByRole('button', { name: 'Done', exact: true }).click(),
    ]);
    assert.equal(response.status(), 200);
    const filteredPage = await response.json();
    assert.ok(filteredPage.items.length > 0);
    assert.ok(filteredPage.items.every((item) => item.harness === fixture.harness));
    await sheet.waitFor({ state: 'hidden' });
    await page.waitForFunction(
      (harness) => new URL(location.href).searchParams.get('harness') === harness,
      fixture.harness,
    );
    await page.locator(`[data-session-row="list"][data-id="${fixture.catalog_key}"]`).waitFor();
  }
  const filterMs = fixture.harness ? performance.now() - started : null,
    filtered = await sample();
  started = performance.now();
  await page.locator(`[data-session-row="list"][data-id="${fixture.catalog_key}"]`).click();
  await page.locator('[data-entry-key]').first().waitFor({ timeout: 60000 });
  const selectedMs = performance.now() - started,
    selected = await sample();
  const entry = await page.locator('[data-entry-key]').first().elementHandle();
  const earlierButton = page.getByRole('button', { name: 'Load earlier records', exact: true });
  await earlierButton.first().waitFor();
  started = performance.now();
  const [earlierResponse] = await Promise.all([
    page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/session-transcript' &&
        new URL(response.url()).searchParams.has('after'),
    ),
    earlierButton.first().click(),
  ]);
  assert.equal(earlierResponse.status(), 200);
  await page.waitForFunction(() => document.querySelectorAll('[aria-busy="true"]').length === 0);
  const earlierMs = performance.now() - started;
  const earlier = await sample();
  assert.ok(earlier.entries > selected.entries, 'Earlier paging must add native records');
  let fieldMs = null;
  if (fixture.scalar) {
    started = performance.now();
    await page.getByRole('button', { name: 'Load recorded text', exact: true }).click();
    await page.getByRole('button', { name: 'Load more text', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Load more text', exact: true }).click();
    await page.getByText('Complete recorded text loaded.', { exact: true }).waitFor();
    fieldMs = performance.now() - started;
    await Promise.all([...fieldCaptures]);
    assert.equal(
      fieldTexts.join(''),
      fixture.scalar,
      'Native field chunks must reconstruct the source exactly',
    );
  }

  await page.evaluate(() => document.querySelector('#nav [data-go="sessions"]').click());
  await page.locator(`[data-session-row="list"][data-id="${fixture.catalog_key}"]`).waitFor();
  started = performance.now();
  await page.locator(`[data-session-row="list"][data-id="${fixture.catalog_key}"]`).click();
  await page.locator('[data-entry-key]').first().waitFor();
  const warmMs = performance.now() - started;
  assert.equal(await entry.evaluate((node) => node.isConnected), true);
  assert.deepEqual(errors, []);
  assert.equal(
    requests.some((path) => /^\/api\/(model|tx|tool|image)(\?|$)/.test(path)),
    false,
  );
  await page.waitForTimeout(100);
  await Promise.all([...captures]);
  assert.deepEqual(errors, []);
  const evidence = {
    source_revision: fixture.source_revision ?? null,
    binary_sha256: fixture.binary_sha256 ?? null,
    bundle,
    source_key: fixture.source_key,
    catalog_key: fixture.catalog_key,
    phase: fixture.phase ?? null,
    firstVisibleCatalogRowMs,
    paintTiming,
    filterMs,
    selectedMs,
    earlierMs,
    fieldMs,
    warmMs,
    before,
    cold,
    filtered,
    selected,
    earlier,
    warm: await sample(),
    requests,
    responses,
    errors,
  };
  await writeFile(evidencePath, JSON.stringify(evidence, null, 2));
  await writeFile(evidencePath + '.result', JSON.stringify({ ok: true }));
  console.log(
    JSON.stringify({
      firstVisibleCatalogRowMs,
      filterMs,
      selectedMs,
      earlierMs,
      fieldMs,
      warmMs,
      bundle,
      errors,
    }),
  );
} catch (error) {
  await writeFile(
    evidencePath + '.result',
    JSON.stringify({
      ok: false,
      error: error.message,
      stack: error.stack,
      requests,
      responses,
      page_errors: errors,
    }),
  );
  throw error;
} finally {
  await browser.close();
}

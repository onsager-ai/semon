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
  responses = [],
  catalogPages = [],
  measurements = {};
let page;
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  page = await context.newPage();
  const cdp = await context.newCDPSession(page);
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
      url.searchParams.delete('t');
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
      if (url.pathname === '/api/sessions' && response.status() === 200 && body) {
        const catalog = JSON.parse(body.toString('utf8'));
        catalogPages.push({
          path: url.pathname + url.search,
          machine: catalog.machine,
          completeness: catalog.completeness,
          search: catalog.search,
          rows: catalog.items.slice(0, 60).map(({ key, harness, last, machine }) => ({
            key,
            harness,
            last,
            source_key: machine?.key ?? catalog.machine,
          })),
        });
        // Keep the initial publication and the latest bounded progression.
        if (catalogPages.length > 20) catalogPages.splice(1, 1);
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
  Object.assign(measurements, { firstVisibleCatalogRowMs, paintTiming });
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
  }
  const filterMs = fixture.harness ? performance.now() - started : null,
    filtered = await sample();
  Object.assign(measurements, { filterMs });
  // Metadata search selects the fixture explicitly; the first 60-row page does
  // not establish that a particular session has been published or is visible.
  const query = page.getByRole('searchbox', { name: 'Search session details', exact: true });
  started = performance.now();
  await query.fill(fixture.catalog_key);
  const [searchResponse] = await Promise.all([
    page.waitForResponse(
      async (response) => {
        const url = new URL(response.url());
        if (
          url.pathname !== '/api/sessions' ||
          url.searchParams.get('q') !== fixture.catalog_key ||
          url.searchParams.get('machine') !== fixture.source_key ||
          (fixture.harness && url.searchParams.get('harness') !== fixture.harness)
        )
          return false;
        if (response.status() !== 200) return true;
        const catalog = await response.json();
        // The reader refreshes partial publications and incomplete metadata
        // searches itself. Observe that progress without issuing extra reads.
        return (
          catalog.items.some((item) => item.key === fixture.catalog_key) ||
          (catalog.completeness.state === 'complete' && catalog.search?.index_complete !== false)
        );
      },
      { timeout: 60000 },
    ),
    query.press('Enter'),
  ]);
  assert.equal(searchResponse.status(), 200);
  const searchPage = await searchResponse.json(),
    selectedItem = searchPage.items.find((item) => item.key === fixture.catalog_key);
  const selectedCatalogReadyMs = performance.now() - started;
  Object.assign(measurements, { selectedCatalogReadyMs });
  assert.ok(selectedItem, 'Metadata search must return the exact fixture catalog key');
  assert.equal(selectedItem.machine?.key ?? searchPage.machine, fixture.source_key);
  if (fixture.harness) assert.equal(selectedItem.harness, fixture.harness);
  // Match the reader's catalogSessionKey source-qualified DOM identity.
  const rowId = selectedItem.machine
      ? JSON.stringify([selectedItem.machine.key, selectedItem.key])
      : selectedItem.key,
    selectedRow = page.locator(`[data-session-row="list"][data-id=${JSON.stringify(rowId)}]`);
  await selectedRow.waitFor();
  const searchMs = performance.now() - started;
  Object.assign(measurements, { searchMs, selectedRowId: rowId });
  started = performance.now();
  await selectedRow.click();
  await page.locator('#page [data-entry-key]:visible').first().waitFor({ timeout: 60000 });
  const selectedMs = performance.now() - started,
    selected = await sample();
  Object.assign(measurements, { selectedMs });
  const entry = await page.locator('#page [data-entry-key]:visible').first().elementHandle();
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
  Object.assign(measurements, { earlierMs });
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
    Object.assign(measurements, { fieldMs });
    await Promise.all([...fieldCaptures]);
    assert.equal(
      fieldTexts.join(''),
      fixture.scalar,
      'Native field chunks must reconstruct the source exactly',
    );
  }

  await page.evaluate(() => document.querySelector('#nav [data-go="sessions"]').click());
  await selectedRow.waitFor();
  started = performance.now();
  await selectedRow.click();
  await page.locator('#page [data-entry-key]:visible').first().waitFor();
  const warmMs = performance.now() - started;
  Object.assign(measurements, { warmMs });
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
    selectedRowId: rowId,
    phase: fixture.phase ?? null,
    firstVisibleCatalogRowMs,
    paintTiming,
    filterMs,
    searchMs,
    selectedCatalogReadyMs,
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
    catalogPages,
    errors,
  };
  await writeFile(evidencePath, JSON.stringify(evidence, null, 2));
  await writeFile(evidencePath + '.result', JSON.stringify({ ok: true }));
  console.log(
    JSON.stringify({
      firstVisibleCatalogRowMs,
      filterMs,
      searchMs,
      selectedCatalogReadyMs,
      selectedMs,
      earlierMs,
      fieldMs,
      warmMs,
      bundle,
      errors,
    }),
  );
} catch (error) {
  const browserState = page
    ? await page
        .evaluate(() => ({
          path: location.pathname,
          harness: new URL(location.href).searchParams.get('harness'),
          q: new URL(location.href).searchParams.get('q'),
          rows: [...document.querySelectorAll('[data-session-row="list"]')]
            .slice(0, 60)
            .map((node) => ({
              id: node.getAttribute('data-id'),
              harness: node.querySelector('[data-harness]')?.getAttribute('data-harness') ?? null,
            })),
        }))
        .catch((failure) => ({ unavailable: failure.message }))
    : null;
  await writeFile(
    evidencePath + '.result',
    JSON.stringify({
      ok: false,
      error: error.message,
      stack: error.stack,
      source_revision: fixture.source_revision ?? null,
      binary_sha256: fixture.binary_sha256 ?? null,
      phase: fixture.phase ?? null,
      measurements,
      catalogPages,
      browserState,
      requests,
      responses,
      page_errors: errors,
    }),
  );
  throw error;
} finally {
  await browser.close();
}

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
fixture.catalog_key ??= fixture.sid;
const evidencePath = process.argv[3] ?? fixture.evidence_path;
await writeFile(
  evidencePath + '.ready',
  JSON.stringify({ pid: process.pid, phase: fixture.phase ?? null }),
);
const browser = await chromium.launch();
try {
  const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  const page = await context.newPage(),
    cdp = await context.newCDPSession(page);
  await cdp.send('Performance.enable');
  const errors = [],
    requests = [],
    responses = [],
    fieldTexts = [];
  const captures = new Set(),
    fieldCaptures = new Set();
  let bundle = null;
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/api/')) requests.push(url.pathname + url.search);
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
      responses.push({
        path: url.pathname,
        status: response.status(),
        bytes,
        ...(unavailable ? { body_unavailable: unavailable } : {}),
        serverResponseMs: started.responseStart - started.requestStart,
      });
    })().catch((error) => errors.push('Response capture failed: ' + error.message));
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
  const coldMs = performance.now() - started,
    cold = await sample();
  if (fixture.harness) {
    started = performance.now();
    const response = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/sessions' &&
        new URL(response.url()).searchParams.get('harness') === fixture.harness,
    );
    await page.getByRole('textbox', { name: 'Harness', exact: true }).fill(fixture.harness);
    await page.getByRole('textbox', { name: 'Harness', exact: true }).press('Tab');
    await response;
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
  let earlierMs = null;
  if (await page.getByRole('button', { name: 'Load earlier records' }).count()) {
    started = performance.now();
    const response = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/session-transcript' &&
        new URL(response.url()).searchParams.has('after'),
    );
    await page.getByRole('button', { name: 'Load earlier records' }).first().click();
    await response;
    await page.locator('[aria-busy="true"]').waitFor({ state: 'detached' });
    earlierMs = performance.now() - started;
  }
  const earlier = await sample();
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
    bundle,
    source_key: fixture.source_key,
    catalog_key: fixture.catalog_key,
    phase: fixture.phase ?? null,
    coldMs,
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
    JSON.stringify({ coldMs, filterMs, selectedMs, earlierMs, fieldMs, warmMs, bundle, errors }),
  );
} catch (error) {
  await writeFile(evidencePath + '.result', JSON.stringify({ ok: false, error: error.message }));
  throw error;
} finally {
  await browser.close();
}

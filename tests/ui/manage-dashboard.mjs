import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { ENV, context, launch } from './lib.mjs';

const fixture = JSON.parse(
  await readFile(new URL('./fixtures/manage-snapshot.json', import.meta.url)),
);
const writer = process.env.SEMON_MANAGE_TOKEN;
assert.ok(writer && ENV.token && ENV.base);
const api = '/api/viewer/manage';
const browser = await launch();
const ctx = await context(browser, { viewport: { width: 360, height: 800 } });
try {
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  assert.equal((await page.request.get(ENV.base + '/manage')).status(), 403);
  assert.equal((await page.request.get(ENV.base + api)).status(), 403);
  await page.goto(`${ENV.base}/manage?t=${ENV.token}`);
  await page.getByText('No snapshot received.').waitFor();
  assert.equal(new URL(page.url()).pathname, '/manage');
  assert.equal(new URL(page.url()).search, '');
  const post = async (data) => {
    const response = await fetch(ENV.base + '/v1/manage/snapshot', {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${writer}` },
      body: JSON.stringify(data),
    });
    assert.equal(response.status, 204);
  };
  await post(fixture);
  await post({ ...fixture, generated_at: '2026-01-01T16:29:00Z', hosts: [] });
  const confirmed = await (await page.request.get(ENV.base + api)).json();
  assert.deepEqual(confirmed.snapshot, fixture);

  // Keep the actual authenticated response and its live Catalog result. Move
  // only receiver time to exercise exact status thresholds in one browser run.
  let age = 89;
  let invalidDate = false;
  await page.route(ENV.base + api, async (route) => {
    const response = await route.fetch();
    assert.equal(response.status(), 200);
    const payload = await response.json();
    payload.now = Date.parse(fixture.generated_at) + age * 60_000;
    if (invalidDate) payload.snapshot.hosts[0].observed_at = '2016-12-31T23:59:60Z';
    await route.fulfill({ response, json: payload });
  });
  for (const [minutes, label, color] of [
    [-1, 'Current', 'green'],
    [89, 'Current', 'green'],
    [90, 'Aging', 'amber'],
    [179, 'Aging', 'amber'],
    [180, 'Stale', 'red'],
  ]) {
    age = minutes;
    await page.reload();
    const status = page.locator(`.manage-dashboard header .manage-${color}`);
    await status.waitFor();
    assert.equal(await status.textContent(), label);
    assert.match(
      await page.locator('.manage-dashboard header').textContent(),
      new RegExp(`${Math.max(0, age)} min old`),
    );
    assert.equal(await page.locator('.manage-dashboard section').count(), 5);
    assert.equal(await page.getByText('Extend the fixture set?').count(), 1);
    assert.equal(await page.getByText('Forecast unavailable', { exact: true }).count(), 1);
    assert.equal(await page.getByText('Prior Shanghai day.').count(), 0);
    assert.equal(await page.getByText('Deferred the optional fixture batch.').count(), 1);
    assert.equal(await page.getByText('Stale heartbeat').count(), Number(age >= 90));
    assert.equal(await page.getByText('64 °C', { exact: false }).count(), 1);
    assert.equal(await page.getByRole('heading', { name: 'Stalled sessions' }).count(), 1);
    assert.ok(
      await page
        .locator('.manage-dashboard')
        .evaluate(
          (element) =>
            element.scrollWidth <= element.clientWidth &&
            document.documentElement.scrollWidth <= innerWidth &&
            Array.from(element.querySelectorAll('p,h2,h3,li,a')).every(
              (node) => parseFloat(getComputedStyle(node).fontSize) >= 15,
            ),
        ),
    );
    if (age === 90 || age === 180) {
      await page.screenshot({ path: `${ENV.out}/manage-${color}-360.png`, fullPage: true });
    }
  }
  invalidDate = true;
  await page.reload();
  await page.getByText('2016-12-31T23:59:60Z', { exact: false }).waitFor();
  assert.deepEqual(errors, []);
} finally {
  await ctx.close();
  await browser.close();
}

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
  const pairing = page.waitForResponse((response) =>
    response.url().endsWith('/api/viewer/pairing'),
  );
  await page.goto(`${ENV.base}/manage?t=${ENV.token}`);
  await pairing;
  await page.getByText('No snapshot received.').waitFor();
  assert.equal(await page.getByRole('button', { name: 'Pair a machine' }).count(), 0);
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
  const response = await page.request.get(ENV.base + api);
  assert.equal(response.status(), 200);
  const confirmed = await response.json();
  assert.deepEqual(confirmed.snapshot, fixture);

  // Keep the actual authenticated response and its live Catalog result. Move
  // only receiver time to exercise exact status thresholds in one browser run.
  let age = 89;
  let invalidDate = false;
  let overflow = false;
  await page.route(ENV.base + api, async (route) => {
    const payload = structuredClone(confirmed);
    payload.now = Date.parse(fixture.generated_at) + age * 60_000;
    if (invalidDate) payload.snapshot.hosts[0].observed_at = '2016-12-31T23:59:60Z';
    if (overflow) {
      const [lane] = payload.snapshot.lanes;
      lane.alerts = ['Alert one', 'Alert two', 'Alert three', 'Alert four'];
      const [decision] = payload.snapshot.decisions_waiting;
      payload.snapshot.decisions_waiting = ['One', 'Two', 'Three', 'Four'].map((suffix) => ({
        ...decision,
        id: `fixture-${suffix}`,
        question: `Decision ${suffix}`,
      }));
    }
    await route.fulfill({ status: 200, json: payload });
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
    const alerts = page.getByRole('region', { name: 'Lane alerts' });
    const lanes = page.getByRole('region', { name: 'Lane health' });
    const fixtureLane = lanes.locator('details').filter({ hasText: 'Eval · Fixtures' });
    const unscheduledLane = lanes.locator('details').filter({ hasText: 'Eval · Notes' });
    const budget = page.getByRole('region', { name: 'Spend' }).locator('details');
    const metrics = page.locator('.manage-metrics');
    assert.equal(await alerts.getByText('Review is waiting for a fixture.').isVisible(), true);
    assert.equal(await fixtureLane.getByText('Lane details').isVisible(), false);
    assert.equal(await budget.getByText('Forecast unavailable').isVisible(), false);
    assert.equal(await metrics.getByText('64 °C', { exact: false }).isVisible(), false);
    if (age === 90 || age === 180) {
      await page.screenshot({ path: `${ENV.out}/manage-${color}-360.png`, fullPage: true });
    }
    await fixtureLane.locator('summary').press('Enter');
    await budget.locator('summary').click();
    await metrics.locator('summary').click();
    assert.equal(await page.getByText('Extend the fixture set?').count(), 1);
    assert.equal(await budget.getByText('Forecast unavailable', { exact: true }).isVisible(), true);
    assert.equal(await page.getByText('Prior Shanghai day.').count(), 0);
    assert.equal(await page.getByText('Deferred the optional fixture batch.').count(), 1);
    assert.equal(await page.getByText('Stale heartbeat').count(), Number(age >= 90));
    assert.match(
      await unscheduledLane.locator('summary').textContent(),
      /Write the fixture notes · no scheduled due/,
    );
    assert.equal(await unscheduledLane.getByText('Overdue', { exact: true }).count(), 0);
    assert.equal(await page.getByText('Overdue', { exact: true }).count(), Number(age > 30));
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
  }
  invalidDate = true;
  await page.reload();
  await page.locator('.manage-metrics summary').click();
  await page.getByText('2016-12-31T23:59:60Z', { exact: false }).waitFor();

  overflow = true;
  await page.reload();
  const alerts = page.getByRole('region', { name: 'Lane alerts' });
  const waiting = page.getByRole('region', { name: 'Waiting on the owner' });
  await waiting.getByRole('heading', { name: 'Decision Three' }).waitFor();
  assert.equal(await alerts.getByRole('listitem').count(), 3);
  assert.equal(await alerts.getByText('Alert four').count(), 0);
  assert.equal(await waiting.getByRole('listitem').count(), 3);
  assert.equal(await waiting.getByText('Decision Four').isVisible(), false);
  await waiting.locator('summary').click();
  assert.equal(await waiting.getByText('Decision Four').isVisible(), true);
  await page.getByRole('region', { name: 'Lane health' }).locator('summary').first().click();
  assert.equal(await page.getByText('Alert four').isVisible(), true);
  assert.deepEqual(errors, []);
} finally {
  await ctx.close();
  await browser.close();
}

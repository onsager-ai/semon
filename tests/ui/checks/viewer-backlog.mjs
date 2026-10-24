// Regressions for the viewer backlog: URL search, shared rows, permission inbox,
// sheet scroll preservation and trace navigation.
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { ENV, launch, served, data, goto, pickFilter } from '../lib.mjs';

const ready = (page, view) =>
  page.waitForFunction(
    (v) =>
      history.state?.v === v &&
      !document.querySelector('#page[aria-busy="true"]') &&
      document.querySelector('#topbar .t') &&
      (v !== 'trace' || document.querySelector('#page .trace-summary')),
    view,
  );
const closePage = async (page) => {
  await page.unrouteAll({ behavior: 'ignoreErrors' });
  await page.context().close();
};

export default async function viewerBacklog(browser) {
  const D = await data();
  for (const size of ['phone', 'desktop']) {
    const page = await served(browser, { size });
    try {
      await ready(page, 'sessions');
      await page.evaluate(() => document.querySelector('.nav-item[data-go="sessions"]').click());
      await ready(page, 'sessions');
      await page.locator('#sq').fill('harbor');
      // Read the browser's URL directly: Playwright's frame URL cache can lag
      // a synchronous replaceState when typing intentionally starts no request.
      assert.equal(
        await page.evaluate(() => new URL(location.href).searchParams.get('q')),
        'harbor',
      );
      await page.locator('#sq').fill('');
      assert.equal(await page.evaluate(() => new URL(location.href).searchParams.has('q')), false);
      await page.reload();
      await ready(page, 'sessions');
      assert.equal(await page.locator('#sq').inputValue(), '');
      await page.waitForTimeout(100);
      const rows = await page.locator('[data-session-row="list"]').evaluateAll((rows) =>
        rows.map((row) => {
          const meta = row.querySelector('.session-row-meta');
          return {
            main: !!row.querySelector('.session-row-main'),
            model: meta.querySelector('.row-model').textContent,
            fits: meta.scrollWidth <= meta.clientWidth + 1,
            items: [...meta.children]
              .filter((x) => !x.hidden)
              .map((x) => ({ cls: x.className, cut: x.scrollWidth > x.clientWidth + 1 })),
          };
        }),
      );
      assert(
        rows.length > 0 &&
          rows.every((x) => x.main && x.model && x.fits && x.items.every((v) => !v.cut)),
        size + ': metadata must drop complete values',
      );
      const children = await page
        .locator('[data-session-row="list"]:has(.row-delegation)')
        .evaluateAll((rows) =>
          rows.map((row) => ({
            main: !!row.querySelector('.session-row-main'),
            name: row.querySelector('.nm')?.textContent,
            status: row.querySelector('.dot')?.getAttribute('aria-label'),
            identity: row.getAttribute('data-id'),
            delegation: row.querySelectorAll('.row-delegation').length === 1,
            metadata: !!row.querySelector('.session-row-meta'),
            fits: row.scrollWidth <= row.clientWidth + 1,
          })),
        );
      assert(
        children.length > 0 &&
          children.every(
            (row) =>
              row.main &&
              row.name &&
              row.status &&
              row.identity &&
              row.delegation &&
              row.metadata &&
              row.fits,
          ),
        size + ': complete child rows retain names, status, metadata and canonical navigation',
      );
      assert(
        (await page.locator('#lanes [data-session-row="compact"] .session-row-main').count()) > 0,
      );
      const fields = await page
        .locator(
          '.session-row-meta > .row-duration, .session-row-meta > .row-machine, .session-row-meta > .repo-short',
        )
        .evaluateAll((fields) =>
          fields.map((field) => ({
            icon: field.querySelectorAll(':scope > svg[aria-hidden="true"]').length,
            text: field.querySelector(':scope > .field-value')?.textContent,
            atomic: field.classList.contains('row-field') && field.hasAttribute('data-drop'),
          })),
        );
      assert(
        fields.length > 0 &&
          fields.every((field) => field.icon === 1 && field.text && field.atomic),
        size + ': metadata fields keep their decorative icon and text together',
      );
      if (size === 'phone') {
        await goto(page, { v: 'analytics' }, D);
        await page.waitForSelector('#page [data-analytics-ready]');
        await page.locator('.chart-slices summary').first().click();
        const slices = page.locator('.chart-slices').first().locator('.chart-slice');
        assert((await slices.count()) > 0, 'touch chart has equivalent slice targets');
        assert(
          await slices.evaluateAll((buttons) =>
            buttons.every((b) => b.getBoundingClientRect().height >= 44),
          ),
          'slice targets are finger-sized',
        );
        await slices.first().click();
        await page.waitForSelector('dialog[open]');
        await page.keyboard.press('Escape');
        await page.waitForFunction(
          () => !document.querySelector('dialog[open]') && !history.state?.sheet,
        );
      }
      await goto(page, { v: 'sessions' }, D);
      for (const [name, value] of [
        ['harness', 'claude'],
        ['repo', 'harbor'],
      ]) {
        await pickFilter(page, name === 'harness' ? 'Harness' : 'Repo', value);
        await ready(page, 'sessions');
        assert.equal(
          new URL(page.url()).searchParams.get(name),
          value,
          'source filters are bookmarked in the canonical URL',
        );
        await pickFilter(page, name === 'harness' ? 'Harness' : 'Repo', '');
        await ready(page, 'sessions');
      }
      const sid = Object.keys(D.TX).find((sid) => D.TX[sid].length > 20) ?? Object.keys(D.SESS)[0];
      await goto(page, { v: 'session', id: sid }, D);
      if (size === 'phone') {
        for (const close of ['button', 'escape', 'back']) {
          await page.evaluate(() =>
            window.scrollTo(0, Math.min(400, document.documentElement.scrollHeight - innerHeight)),
          );
          const before = await page.evaluate(() => scrollY);
          await page.locator('#more-btn').click();
          await page.waitForTimeout(50);
          assert.equal(
            await page.evaluate(() => scrollY),
            before,
            'opening details must preserve transcript scroll',
          );
          if (close === 'button') await page.locator('.session-menu .panel-h button').click();
          else if (close === 'escape') await page.keyboard.press('Escape');
          else await page.goBack();
          await page.waitForFunction(
            () => !document.querySelector('dialog[open]') && !history.state?.sheet,
          );
          await page.waitForTimeout(100);
          assert.equal(
            await page.evaluate(() => scrollY),
            before,
            close + ' must restore the same scroll',
          );
        }
      }
      const trace = D.turns.find((t) => t.sent.length && D.SESS[t.sid]);
      assert(trace, 'fixture needs an outgoing trace');
      await goto(page, { v: 'session', id: trace.sid, turn: trace.id }, D);
      await page.locator('#page .turn[data-turn="' + trace.id + '"]').scrollIntoViewIfNeeded();
      await page.locator('#more-btn').click();
      const traceIcon = await page.locator('.menu-trace > svg').boundingBox();
      assert(
        traceIcon && traceIcon.width === 18 && traceIcon.height === 18,
        'the Trace menu icon must match the compact action icons',
      );
      await page.locator('.menu-trace').click();
      await ready(page, 'trace');
      assert.equal(await page.locator('#topbar .t').textContent(), 'Trace');
      assert.equal(await page.locator('#topbar #more-btn').count(), 1);
      assert.equal(await page.locator('#topbar .trace-back').count(), 1);
      assert.equal(await page.locator('#page .trace-summary').count(), 1);
      assert.equal(await page.locator('#topbar .meta-line').count(), 0);
      assert.equal(page.errors.length, 0, page.errors.join(' | '));
    } finally {
      await closePage(page);
    }
  }

  // Legacy inbox metadata must not restore the removed Home surface.
  const page = await served(browser, { size: 'desktop' });
  try {
    const home = await (await page.request.get(ENV.base + '/api/viewer/home')).json(),
      row = structuredClone(home.recent[0]),
      waitId = 'wait:' + JSON.stringify([row.source_key, row.catalog_key]);
    row.session.state = 'wait';
    row.session.waiting_for = 'permission prompt';
    row.event = {
      id: waitId,
      at: home.now,
      done: null,
      ask: 'decision',
      status: 'wait',
      brief: 'permission prompt',
      answer: null,
      declined: false,
    };
    home.inbox = home.inbox.filter((item) => item.catalog_key !== row.catalog_key);
    home.inbox.push(row);
    home.totals.waiting = home.inbox.length;
    home.version += '-wait';
    const retired = [];
    page.on('request', (request) => {
      if (/\/api\/(model|tx|events)(?:[?/#]|$)/.test(request.url())) retired.push(request.url());
    });
    await page.route('**/api/viewer/home**', (r) => r.fulfill({ json: home }));
    await page.reload();
    await ready(page, 'sessions');
    const inbox = page.locator('[data-h]').filter({ hasText: 'permission prompt' });
    assert.equal(await inbox.count(), 0, 'Home inbox must not move into Sessions');
    assert.equal(await page.locator('#nav [data-go="home"]').count(), 0);
    assert.ok(await page.locator('#lanes .srow').count(), 'Recent remains available');
    const versions = [];
    await page.route('**/api/sessions?**', async (route) => {
      const response = await route.fetch();
      if (response.status() !== 200) return route.fulfill({ response });
      const catalog = await response.json();
      if (!new URL(route.request().url()).searchParams.has('sid')) {
        const keys = new Set(catalog.items.map((item) => item.key));
        const roots = catalog.items.filter((item) => !item.parent || !keys.has(item.parent));
        assert(roots.length >= 2, 'fixture needs two root model labels');
        versions.splice(0, versions.length, ...roots.slice(0, 2).map((item) => item.key));
        roots[0].model = 'gpt-6-sol';
        roots[1].model = 'gpt-6.1-sol';
      }
      await route.fulfill({ response, json: catalog });
    });
    // Re-enter through a fresh document so this interceptor observes a real
    // Catalog read instead of the current Sessions page's retained cache.
    const modeled = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return url.pathname === '/api/sessions' && !url.searchParams.has('sid') && response.ok();
    });
    await page.reload();
    await modeled;
    await ready(page, 'sessions');
    assert.equal(versions.length, 2, 'model-label fixture read did not complete');
    assert.equal(
      await page.locator('.nrow[data-id="' + versions[0] + '"] .row-model').textContent(),
      'sol',
    );
    assert.equal(
      await page.locator('.nrow[data-id="' + versions[1] + '"] .row-model').textContent(),
      'sol 6.1',
    );
    assert.equal(retired.length, 0);
  } finally {
    await closePage(page);
  }

  console.log('viewer backlog regressions passed');
  return true;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  try {
    await viewerBacklog(browser);
  } finally {
    await browser.close();
  }
}

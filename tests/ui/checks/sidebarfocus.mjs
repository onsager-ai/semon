import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { launch, served, data } from '../lib.mjs';

export default async function sidebarFocus(browser) {
  const D = await data(),
    page = await served(browser, { size: 'desktop' });
  try {
    const response = await page.request.get(new URL('/api/viewer/home', page.url()).href);
    assert.equal(response.status(), 200);
    const home = await response.json(),
      base = home.recent[0],
      source = base.source_key,
      key = 'focus-parent',
      parent = JSON.stringify([source, key]);
    home.recent = [
      {
        ...base,
        id: parent,
        catalog_key: key,
        session: {
          ...base.session,
          name: 'Focus parent',
          parent: null,
          state: 'work',
          last: home.now + 1,
          content_generation: null,
        },
      },
    ];
    for (let i = 0; i < 40; i++) {
      const child = 'focus-child-' + i;
      home.recent.push({
        ...base,
        id: JSON.stringify([source, child]),
        catalog_key: child,
        session: {
          ...base.session,
          name: 'Focus child ' + i,
          parent: key,
          state: i === 0 ? 'wait' : 'work',
          last: home.now - i,
          content_generation: null,
        },
      });
    }
    home.version += '-focus';
    home.observations = [];
    await page.route('**/api/viewer/home**', (route) => route.fulfill({ json: home }));
    await page.reload();
    await page.waitForFunction(() => history.state?.v === 'home');
    const item = page.getByRole('treeitem', { name: 'Focus parent', exact: true });
    await item.locator('.tree-all').first().click();
    await item
      .locator(':scope > .tree-group > .treeitem')
      .first()
      .evaluate((child) => child.focus());
    await page.keyboard.press('Shift+Tab');
    const focus = await item.evaluate((item) => {
      const row = item.querySelector(':scope > .tree-row'),
        r = row.getBoundingClientRect(),
        list = document.querySelector('#side-list').getBoundingClientRect();
      return {
        inParent: row.contains(document.activeElement),
        visible: r.top >= list.top - 1 && r.bottom <= list.bottom + 1,
      };
    });
    assert(
      focus.inParent && focus.visible,
      'Shift+Tab from the first child must reach the visible pinned parent row',
    );
    await item.locator(':scope > .tree-row .tree-fewer').click();
    const folded = await item.locator(':scope > .tree-row .srow').evaluate((row) => {
      const r = row.getBoundingClientRect(),
        list = document.querySelector('#side-list').getBoundingClientRect();
      return {
        focused: document.activeElement === row,
        visible: r.top >= list.top - 1 && r.bottom <= list.bottom + 1,
        padding: parseFloat(
          getComputedStyle(document.querySelector('#side-list')).scrollPaddingTop,
        ),
        height: r.height,
      };
    });
    assert(folded.focused && folded.visible, 'Show fewer must focus a visible parent');
    assert(
      folded.padding >= folded.height,
      'scroll padding must clear the actual parent row height',
    );
    assert.match(
      await item.locator(':scope > .tree-row .kid-flag').getAttribute('data-tip'),
      /1 needs you/,
    );
    await page.locator('#rail-toggle').click();
    assert(
      await page
        .locator('#lanes > .treeitem > .tree-row')
        .evaluateAll((rows) => rows.every((r) => getComputedStyle(r).position !== 'sticky')),
      'rail rows never pin',
    );
    await page.setViewportSize({ width: 1280, height: 420 });
    for (const scheme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme: scheme });
      // A rail toggle changes the app grid over 180 ms. Sample its complete hit
      // area after that actual transition, rather than a transient logo overlap.
      await page.locator('.app').evaluate(async (app) => {
        getComputedStyle(app).gridTemplateColumns;
        await Promise.all(app.getAnimations().map((animation) => animation.finished));
      });
      const hit = await page.locator('#rail-toggle').evaluate((button) => {
        const rect = button.getBoundingClientRect();
        return {
          rect: { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom },
          hits: [
            [rect.left + rect.width / 2, rect.top + rect.height / 2],
            [rect.left + 1, rect.top + rect.height / 2],
            [rect.right - 1, rect.top + rect.height / 2],
          ].map(([x, y]) => {
            const element = document.elementFromPoint(x, y);
            return {
              owned: button.contains(element),
              tag: element?.tagName,
              id: element?.id,
              className: element?.getAttribute('class'),
            };
          }),
        };
      });
      assert(
        hit.hits.every((point) => point.owned),
        scheme + ': short-window rail toggle must own its hit area: ' + JSON.stringify(hit),
      );
      const before = await page.locator('#rail-toggle').getAttribute('aria-expanded');
      await page.locator('#rail-toggle').click();
      assert.notEqual(await page.locator('#rail-toggle').getAttribute('aria-expanded'), before);
    }
    assert.equal(page.errors.length, 0, page.errors.join(' | '));
  } finally {
    await page.unrouteAll({ behavior: 'ignoreErrors' });
    await page.context().close();
  }
  console.log('Pinned parent focus regressions passed');
  return true;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const browser = await launch();
  try {
    await sidebarFocus(browser);
  } finally {
    await browser.close();
  }
}

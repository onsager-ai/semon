import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { launch, served, data } from '../lib.mjs';

export default async function sidebarFocus(browser) {
  const D = await data(),
    page = await served(browser, { size: 'desktop' });
  try {
    const model = structuredClone(D.model),
      base = Object.values(model.sessions).find((s) => s.lane),
      parent = 'focus-parent';
    model.sessions[parent] = {
      ...base,
      name: 'Focus parent',
      parent: null,
      kind: undefined,
      lane: true,
      state: 'work',
      last: model.now + 1,
    };
    for (let i = 0; i < 40; i++)
      model.sessions['focus-child-' + i] = {
        ...base,
        name: 'Focus child ' + i,
        parent,
        kind: 'Subagent',
        lane: false,
        state: i === 0 ? 'wait' : 'work',
        last: model.now - i,
      };
    model.version += '-focus';
    await page.route('**/api/model**', (route) => route.fulfill({ json: model }));
    await page.reload();
    await page.waitForFunction(() => history.state?.v === 'home');
    const item = page.locator('#lanes > .treeitem[data-id="' + parent + '"]');
    await item.locator('.tree-all[data-id="' + parent + '"]').click();
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
      const hit = await page.locator('#rail-toggle').evaluate((button) => {
        const rect = button.getBoundingClientRect();
        return [
          [rect.left + rect.width / 2, rect.top + rect.height / 2],
          [rect.left + 1, rect.top + rect.height / 2],
          [rect.right - 1, rect.top + rect.height / 2],
        ].every(([x, y]) => button.contains(document.elementFromPoint(x, y)));
      });
      assert(hit, scheme + ': short-window rail toggle must own its hit area');
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

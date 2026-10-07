import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { chromium } from '../../tests/ui/node_modules/playwright/index.mjs';
import { readFile } from 'node:fs/promises';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/lib/select.tsx', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'iife',
  globalName: 'SelectOwner',
  platform: 'browser',
});
const css = await readFile(
  new URL('../../crates/semon-sessions/src/select.css', import.meta.url),
  'utf8',
);
async function fixture(run) {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
    await page.route('http://select.test/**', (route) =>
      route.fulfill({
        contentType: 'text/html',
        body: '<!doctype html><style>' + css + '</style><div id="mount"></div>',
      }),
    );
    page.setDefaultTimeout(5000);
    await page.goto('http://select.test/');
    await page.addScriptTag({ content: outputFiles[0].text });
    await page.evaluate(() => {
      SelectOwner.installSelect();
      window.owner = SelectOwner.createSelect({
        label: 'Options',
        searchAbove: 100,
        options: Array.from({ length: 40 }, (_, i) => ({ value: String(i), label: 'Option ' + i })),
      });
      document.querySelector('#mount').append(owner.el);
    });
    await run(page);
  } finally {
    await browser.close();
  }
}
test('orphaned Select closes its sheet and removes its own history entry', async () =>
  fixture(async (page) => {
    await page.evaluate(() => owner.open());
    assert.equal(await page.locator('dialog[open]').count(), 1);
    await page.evaluate(() => {
      owner.el.remove();
      document.body.dispatchEvent(
        new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: 10 }),
      );
    });
    await page.waitForFunction(() => !history.state?.shSelect);
    assert.equal(await page.locator('dialog[open]').count(), 0);
    assert.equal(await page.evaluate(() => owner.isOpen), false);
    const prevented = await page.evaluate(
      () =>
        !document.body.dispatchEvent(
          new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: 10 }),
        ),
    );
    assert.equal(prevented, false);
    await page.evaluate(() => owner.destroy());
  }));
test('Select permits touch reversal at the list end and pinch zoom', async () =>
  fixture(async (page) => {
    await page.evaluate(() => owner.open());
    const result = await page.evaluate(() => {
      const list = document.querySelector('.sh-select-list');
      list.style.height = '150px';
      list.style.overflow = 'auto';
      list.scrollTop = list.scrollHeight;
      const touch = (y) => new Touch({ identifier: 1, target: list, clientY: y });
      const send = (type, ys) =>
        list.dispatchEvent(
          new TouchEvent(type, { bubbles: true, cancelable: true, touches: ys.map(touch) }),
        );
      send('touchstart', [100]);
      const blockedDown = !send('touchmove', [50]);
      const reversed = send('touchmove', [70]);
      const pinch = send('touchmove', [60, 80]);
      return { blockedDown, reversed, pinch, room: list.scrollHeight - list.clientHeight };
    });
    assert.ok(result.room > 1);
    assert.deepEqual(
      { ...result, room: undefined },
      { blockedDown: true, reversed: true, pinch: true, room: undefined },
    );
    await page.evaluate(() => owner.destroy());
  }));
test('Select reopen waits for the previous close history transaction', async () =>
  fixture(async (page) => {
    await page.evaluate(() => {
      owner.open();
      owner.close();
      owner.open();
    });
    await page.waitForFunction(() => owner.isOpen && !!history.state?.shSelect);
    await page.evaluate(() => owner.close());
    await page.waitForFunction(() => !history.state?.shSelect);
    assert.equal(await page.locator('dialog[open]').count(), 0);
    await page.evaluate(() => owner.destroy());
  }));

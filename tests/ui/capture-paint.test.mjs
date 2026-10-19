import assert from 'node:assert/strict';
import { test } from 'node:test';
import { launch } from './lib.mjs';
import { withCapturePaint, captureStableFrame } from './capture-paint.mjs';

test('PNG and evidence share a repeatable phase without masking content or retaining paused effects', async () => {
  const browser = await launch();
  try {
    const page = await browser.newPage();
    await page.setContent(`<style>
      #dot { width: 20px; height: 20px; background: blue; animation: pulse 1s infinite; }
      #panel { transition: opacity .6s; opacity: 0; }
      @keyframes pulse { from { box-shadow: 0 0 0 0 blue; } to { box-shadow: 0 0 0 8px transparent; } }
    </style><div id="dot"></div><div id="panel">Visible result</div>`);
    await page.locator('#panel').evaluate((node) => {
      node.style.opacity = '1';
    });
    const shadow = () => page.locator('#dot').evaluate((node) => getComputedStyle(node).boxShadow);
    const moving = await shadow();
    await page.waitForTimeout(137);
    assert.notEqual(await shadow(), moving, 'the unbounded capture is timing-dependent');
    const shot = () =>
      withCapturePaint(page, async () => ({
        bytes: await captureStableFrame(page, { animations: 'allow', caret: 'hide' }),
        evidence: await page.evaluate(() => ({
          shadow: getComputedStyle(document.querySelector('#dot')).boxShadow,
          opacity: getComputedStyle(document.querySelector('#panel')).opacity,
          text: document.querySelector('#panel').innerText,
          motion: document.getAnimations().map((animation) => ({
            state: animation.playState,
            time: animation.currentTime,
          })),
        })),
      }));
    const first = await shot();
    await page.waitForTimeout(137);
    const second = await shot();
    assert.deepEqual(first, second);
    assert.equal(first.evidence.opacity, '1');
    assert.equal(first.evidence.text, 'Visible result');
    assert.deepEqual(first.evidence.motion, [{ state: 'paused', time: 0 }]);
    assert.equal(await page.evaluate(() => document.getAnimations()[0].playState), 'running');
    await page.locator('#panel').evaluate((node) => {
      node.textContent = 'Different result';
    });
    assert.notDeepEqual((await shot()).bytes, first.bytes);
    await assert.rejects(
      withCapturePaint(page, () => {
        throw new Error('capture failed');
      }),
      /capture failed/,
    );
    assert.equal(await page.evaluate(() => document.getAnimations()[0].playState), 'running');
  } finally {
    await browser.close();
  }
});

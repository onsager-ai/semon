import assert from 'node:assert/strict';
import { test } from 'node:test';
import { launch } from './lib.mjs';
import { withCapturePaint, captureStableFrame } from './capture-paint.mjs';

test('PNG and evidence share a repeatable phase without masking content or retaining paused effects', async () => {
  const browser = await launch({ baselineRaster: true });
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

test('independent visual browsers retain exact rounded-edge and text pixels', async () => {
  const capture = async (label) => {
    const browser = await launch({ baselineRaster: true });
    try {
      const page = await browser.newPage({ viewport: { width: 820, height: 860 } });
      await page.setContent(`<style>
        body { background: #f7f8f5; color: #191d1b; font: 15px/22px sans-serif; }
        aside { position: sticky; top: 0; width: 280px; clip-path: inset(1px 0 0); }
        button { border: 0; border-radius: 9px; background: #f0f2ed;
          width: 278px; height: 48px; text-align: left; color: inherit; }
        article { margin-left: 296px; width: 443.375px; }
        article p { border-radius: 6px; padding: 13.25px; background: #fff; }
        svg { width: 15px; height: 15px; fill: none; stroke: currentColor; }
      </style><aside><button>harbor · Working</button></aside>
      <article><p>${label}</p><svg viewBox="0 0 24 24"><path d="M4 4v8h14m-4-4 4 4-4 4"/></svg></article>`);
      return await withCapturePaint(page, () =>
        captureStableFrame(page, { animations: 'allow', caret: 'hide' }),
      );
    } finally {
      await browser.close();
    }
  };
  const first = await capture('Recorded result');
  assert.deepEqual(await capture('Recorded result'), first);
  assert.notDeepEqual(await capture('Changed result'), first);
});

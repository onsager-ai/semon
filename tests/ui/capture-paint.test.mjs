import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
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
  assert.match(
    readFileSync(new URL('./lib.mjs', import.meta.url), 'utf8'),
    /chromium\.launch\(\s*launchOptions\(\{\s*baselineRaster\s*\}\)\s*\)/,
  );
  const harnessMark = readFileSync(
    new URL('../../assets/harnesses/claude-code.svg', import.meta.url),
  );
  const harnessUrl = 'data:image/svg+xml;base64,' + harnessMark.toString('base64');
  const harnessMarks = Array.from(
    { length: 3 },
    () => `<span class="hicon"><img src="${harnessUrl}" alt="Claude"></span>`,
  ).join('');
  const capture = async (label) => {
    const browser = await launch({ baselineRaster: true });
    try {
      const page = await browser.newPage({ viewport: { width: 820, height: 860 } });
      await page.setContent(`<style>
        body { margin: 0; background: #fff; color: #191d1b; font: 15px/22px sans-serif; }
        aside { position: fixed; inset: 0 auto 0 0; width: 296px;
          background: #f7f7f5; overflow-y: auto; }
        .spacer { height: 293px; }
        .tree { position: sticky; top: 0; margin: 0 4px 0 8px;
          background: #f7f7f5; clip-path: inset(1px 0 0); }
        button { border: 0; border-radius: 9px; background: #f1f2ef;
          width: 284px; height: 49px; text-align: left; color: inherit; }
        .tail { height: 1200px; }
        .marks { position: absolute; left: 16px; top: 565.25px; }
        .hicon { display: flex; box-sizing: border-box; width: 14px; height: 14px;
          padding: 1.75px; margin-bottom: 35px; }
        .hicon img { width: 100%; height: 100%; object-fit: contain; }
        article { margin-left: 296px; width: 443.375px; }
        article p { border-radius: 6px; padding: 13.25px; background: #fff; }
        svg { width: 15px; height: 15px; fill: none; stroke: currentColor; }
      </style><aside><div class="spacer"></div><div class="tree"><button>harbor · Working</button></div><div class="tail"></div>
      <div class="marks">${harnessMarks}</div></aside>
      <article><p>${label}</p><svg viewBox="0 0 24 24"><path d="M4 4v8h14m-4-4 4 4-4 4"/></svg></article>`);
      await page
        .locator('.hicon img')
        .evaluateAll((images) => Promise.all(images.map((image) => image.decode())));
      const shot = () =>
        withCapturePaint(page, () =>
          captureStableFrame(page, { animations: 'allow', caret: 'hide' }),
        );
      const first = await shot();
      for (const offset of [1, 300, 0, 11, 0]) {
        await page.locator('aside').evaluate((node, offset) => {
          node.scrollTop = offset;
        }, offset);
        if (offset === 0)
          assert.deepEqual(await shot(), first, 'repaint retains exact edge pixels');
      }
      return first;
    } finally {
      await browser.close();
    }
  };
  const first = await capture('Recorded result');
  assert.deepEqual(await capture('Recorded result'), first);
  assert.notDeepEqual(await capture('Changed result'), first);
});

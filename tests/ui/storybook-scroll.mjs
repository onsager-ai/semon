// Screenshot identity only. The input sequences are retained from pixels.mjs
// resetTopScroll/prepareFrame and preservation.mjs capture; no reader policy changes.
export function legacyScrollPolicy(comparator, kind, frame, width) {
  if (comparator === 'preservation') return 'preservation-reset';
  if (comparator === 'gallery') return 'gallery-action-position';
  if (kind === 'frame' && frame.startsWith('session-'))
    return width === 390 ? 'document-tail' : 'main-tail';
  if (kind === 'region') return 'region-top';
  return 'top';
}

export async function applyCaptureScroll(page, policy = 'top', { input = true } = {}) {
  await page.evaluate(async () => {
    await document.fonts.ready;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
  if (input && policy === 'preservation-reset') {
    await page.mouse.move(page.viewportSize().width - 20, 300);
    await page.mouse.wheel(0, -100000);
    await page.keyboard.press('Home');
  } else if (input && policy === 'region-top') {
    const point = await page.locator('#topbar').evaluate((bar) => {
      const rect = bar.getBoundingClientRect(),
        x = Math.min(rect.right - 1, innerWidth - 1),
        y = Math.max(rect.top + 1, 1),
        target = document.elementFromPoint(x, y);
      if (!target || !bar.contains(target) || target.closest('button, a, input, [role="button"]'))
        throw Error('Region reading input requires inert topbar padding');
      return { x, y };
    });
    await page.mouse.move(point.x, point.y);
    await page.mouse.wheel(0, -100000);
  }
  if (!['top', 'region-top', 'preservation-reset', 'document-tail', 'main-tail', 'gallery-action-position'].includes(policy))
    throw Error('Unknown capture scroll policy ' + policy);
  await page.evaluate(async (policy) => {
    const main = document.querySelector('#main');
    if (policy !== 'gallery-action-position') {
      window.scrollTo(0, policy === 'document-tail' ? document.documentElement.scrollHeight : 0);
      if (main) main.scrollTop = policy === 'main-tail' ? main.scrollHeight : 0;
    }
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  }, policy);
  const actual = await page.evaluate(() => {
    const main = document.querySelector('#main');
    return {
      window: scrollY,
      documentTail: Math.max(0, document.documentElement.scrollHeight - innerHeight),
      main: main?.scrollTop ?? 0,
      mainTail: main ? Math.max(0, main.scrollHeight - main.clientHeight) : 0,
    };
  });
  const expectedWindow = policy === 'gallery-action-position' ? actual.window : policy === 'document-tail' ? actual.documentTail : 0,
    expectedMain = policy === 'gallery-action-position' ? actual.main : policy === 'main-tail' ? actual.mainTail : 0;
  if (Math.abs(actual.window - expectedWindow) > 1 || Math.abs(actual.main - expectedMain) > 1)
    throw Error('Capture scroll policy did not settle: ' + JSON.stringify({ policy, actual }));
  return { policy, ...actual };
}

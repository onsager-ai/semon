// Keep the PNG and its DOM evidence at the same animation phase. Playwright's
// screenshot-only animation suppression resumes animations before diagnostics
// run, so byte-exact approvals otherwise depend on wall-clock timing.
export async function withCapturePaint(page, capture) {
  const paused = await page.evaluateHandle(async () => {
    const saved = document.getAnimations().map((animation) => ({
      animation,
      currentTime: animation.currentTime,
      playState: animation.playState,
    }));
    for (const { animation } of saved) {
      if (animation.effect?.getComputedTiming().endTime === Infinity) {
        animation.pause();
        animation.currentTime = 0;
      } else if (animation.playState !== 'finished') {
        // Match screenshot({ animations: 'disabled' }): settle finite effects.
        animation.finish();
      }
    }
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return saved;
  });
  try {
    return await capture();
  } finally {
    await paused.evaluate((saved) => {
      for (const { animation, currentTime, playState } of saved) {
        if (animation.effect?.getComputedTiming().endTime !== Infinity) continue;
        animation.currentTime = currentTime;
        if (playState === 'running') animation.play();
      }
    });
    await paused.dispose();
  }
}

// Mobile full-page capture can initially reuse stale compositor tiles after
// scrolling a fixed header/clipped transcript. Require identical consecutive
// paints, rather than accepting a transient blank or selecting a favorable diff.
export async function captureStableFrame(page, options) {
  let previous = await page.screenshot(options);
  for (let attempt = 0; attempt < 4; attempt++) {
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    );
    const current = await page.screenshot(options);
    if (current.equals(previous)) return current;
    previous = current;
  }
  throw new Error('Full-frame screenshot paint did not stabilize');
}

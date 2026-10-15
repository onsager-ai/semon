// Real source-scoped causal runs retain bounded fan-out and native transcript destinations.
import assert from 'node:assert/strict';
import { served, data, goto, closePage, overflow } from '../lib.mjs';

export default async function runView(browser) {
  const D = await data({ extras: true }),
    root = D.turns.find((turn) => turn.sid === 'swarm');
  assert.ok(root);
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
      const page = await served(browser, {
        extras: true,
        size: width === 390 ? 'phone' : 'desktop',
        dark,
        viewport: { width, height: 860 },
      });
      const retired = [],
        reads = [];
      try {
        page.on('request', (request) => {
          const url = new URL(request.url());
          if (/^\/api\/(model|tx|events)(?:\/|$)/.test(url.pathname)) retired.push(url.pathname);
          if (url.pathname === '/api/session-trace') reads.push(url);
        });
        await goto(page, { v: 'trace', sid: root.sid, turn: root.id }, D);
        await page.locator('.agents-panel .agent-row').first().waitFor();
        assert.equal(await page.locator('.agent-more').count(), 1);
        const collapsed = await page.locator('.agent-row[data-agent-id]').count();
        await page.locator('.agent-more').click();
        assert.equal(await page.locator('.agent-row[data-agent-id]').count(), 13);
        assert.ok(collapsed < 13, 'large fan-out is folded before explicit expansion');
        assert.equal(
          await page.evaluate(() =>
            [...document.querySelectorAll('.agent-segment')].some(
              (node) => node.getBoundingClientRect().width < 0,
            ),
          ),
          false,
        );
        assert.equal(
          await page.locator('.agent-row[data-critical="true"]').count(),
          0,
          'a spawn without a recorded target wait cannot imply a critical wait',
        );
        if (width === 390)
          assert.equal(
            await page
              .locator('.agent-row')
              .evaluateAll((rows) =>
                rows.every((node) => node.getBoundingClientRect().height >= 44),
              ),
            true,
          );
        const child = page.locator('.agent-row[data-agent-id="swarm-worker-1"]');
        await child.click();
        await page.locator('[aria-label="Transcript"] [data-entry-key]').first().waitFor();
        assert.equal(new URL(page.url()).pathname.endsWith('/swarm-worker-1'), true);
        assert.equal(
          new URL(page.url()).searchParams.get('machine'),
          reads[0].searchParams.get('machine'),
        );
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
  console.log('Native causal fan-out, recorded waits and source navigation passed');
  return true;
}

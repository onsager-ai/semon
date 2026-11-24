// Trace keeps native sender calls visible without linking to their targets.
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
        previews = [];
      try {
        page.on('request', (request) => {
          const url = new URL(request.url());
          if (/^\/api\/(model|tx|events)(?:\/|$)/.test(url.pathname)) retired.push(url.pathname);
          if (url.pathname === '/api/session-home') previews.push(url);
        });
        await page.route('**/api/session-trace?**', async (route) => {
          const response = await route.fetch();
          if (!response.ok()) return route.fulfill({ response });
          const trace = await response.json();
          for (const edge of trace.handoffs) {
            if (edge.kind !== 'spawn') continue;
            delete edge.reason;
            edge.to = 'swarm-worker-1';
          }
          await route.fulfill({ response, json: trace });
        });
        await goto(page, { v: 'trace', sid: root.sid, turn: root.id }, D);
        const outgoing = page.locator('.hop.child.k-spawn');
        await outgoing.first().waitFor();
        for (const text of await outgoing.allTextContents()) {
          assert.match(text, /not linked yet/);
          assert.doesNotMatch(text, /swarm-worker-1|Worker 1/);
        }
        assert.equal(await outgoing.locator('a, button').count(), 0);
        assert.equal(await page.locator('.agent-row[data-agent-id="swarm-worker-1"]').count(), 0);
        assert.ok(previews.every((url) => url.searchParams.get('sid') === root.sid));
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
  console.log('Native Trace sender actions stayed visible without target navigation');
  return true;
}

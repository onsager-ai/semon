// Native permalinks preserve qualified anchors and independent reading controls.
import assert from 'node:assert/strict';
import { served, data, goto, closePage, overflow } from '../lib.mjs';

export default async function deepLinkLive(browser) {
  const D = await data({ extras: true });
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
          if (url.pathname === '/api/session-transcript' || url.pathname === '/api/session-anchor')
            reads.push(url);
        });
        const turn = D.turns.find((turn) => turn.sid === 'backlog');
        assert.ok(turn);
        await goto(page, { v: 'session', id: 'backlog', turn: turn.id }, D);
        const anchor = page.locator('.turn[data-turn="' + turn.id + '"]');
        await anchor.waitFor();
        assert.equal(decodeURIComponent(new URL(page.url()).hash.slice(1)), turn.id);
        assert.equal(new URL(page.url()).searchParams.has('turn'), false);
        assert.equal(new URL(page.url()).pathname.split('/')[1], 'session');
        const source = decodeURIComponent(new URL(page.url()).pathname.split('/')[2]);
        assert.ok(source);
        assert.ok(
          reads.some(
            (url) =>
              url.pathname === '/api/session-anchor' && url.searchParams.get('turn') === turn.id,
          ),
        );
        assert.ok(
          reads.every(
            (url) =>
              url.searchParams.get('machine') === source &&
              url.searchParams.get('scope') === 'retained_history',
          ),
        );
        const before = await anchor.boundingBox();
        const bar = await page.locator('#topbar').boundingBox();
        assert.ok(
          before.y >= bar.y + bar.height - 1 && before.y < 860 - 40,
          'the native turn lands below the Topbar',
        );
        const url = page.url();
        assert.equal(
          await page.getByRole('button', { name: 'Open Composer', exact: true }).count(),
          0,
        );
        assert.equal(page.url(), url, 'Collection controls preserve the permalink');
        assert.equal(await page.locator('#page [aria-label="Conversation controls"]').count(), 0);
        const unavailable = new URL(page.url());
        unavailable.hash = 'unrecorded-native-turn';
        await page.goto(unavailable.href);
        await page
          .getByText('This recorded turn is unavailable in the retained history.', { exact: true })
          .waitFor();
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
  console.log('Native source-qualified permalinks, anchors and collection control refusal passed');
  return true;
}

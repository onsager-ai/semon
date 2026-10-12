// Qualified native previews keep their typography when expanded and their metadata inside the viewport.
import assert from 'node:assert/strict';
import { served, data, goto, closePage, overflow } from '../lib.mjs';

export default async function tracebrief(browser) {
  const D = await data(),
    turn = D.turns.find((turn) => turn.sid === 'harbor' && turn.sent.length),
    result = D.H.find(
      (edge) => edge.from === 'principal' && edge.kind === 'toyou' && edge.ask === 'result',
    ),
    resultTurn = D.turns.find((turn) => turn.sid === 'principal' && turn.sent.includes(result?.id));
  assert.ok(turn);
  assert.ok(result?.brief && resultTurn, 'recorded closing-result fixture');
  for (const width of [390, 820, 1280])
    for (const dark of [false, true]) {
      const page = await served(browser, {
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
        await page.route('**/api/session-home?**', async (route) => {
          const response = await route.fetch();
          if (!response.ok()) return route.fulfill({ response });
          const data = await response.json();
          for (const value of Object.values(data.events))
            if (value.brief)
              value.brief +=
                '\n\n' +
                'Native preview typography and source identity remain recorded. '.repeat(70);
          await route.fulfill({ response, json: data });
        });
        await goto(page, { v: 'trace', sid: turn.sid, turn: turn.id }, D);
        await page.locator('.hop .body > .more:visible').first().waitFor();
        const more = page.locator('.hop .body > .more:visible').first();
        const style = () =>
          more.evaluate((node) => {
            const brief = node.parentElement.querySelector(':scope > .brief');
            const properties = [
              'color',
              'fontSize',
              'fontFamily',
              'fontWeight',
              'fontStyle',
              'lineHeight',
            ];
            const read = (element) =>
              Object.fromEntries(properties.map((name) => [name, getComputedStyle(element)[name]]));
            return {
              brief: read(brief),
              inner: read(brief.querySelector('p, li')),
              accent: getComputedStyle(node).color,
              open: brief.classList.contains('open'),
            };
          });
        const before = await style();
        await more.click();
        const after = await style();
        assert.equal(after.open, true);
        assert.deepEqual(after.brief, before.brief);
        assert.deepEqual(after.inner, before.inner);
        assert.notEqual(after.brief.color, after.accent);
        await more.click();
        assert.equal((await style()).open, false);
        await goto(page, { v: 'trace', sid: resultTurn.sid, turn: resultTurn.id }, D);
        const closing = page.locator(`.hop[data-h="${result.id}"] > .body > .brief`);
        await closing.waitFor();
        assert.match(
          await page.locator('.hop.k-root .meta').textContent(),
          /Sent you a result/,
          'Trace exposed an internal turn-end code instead of its recorded outcome',
        );
        assert.ok(
          (await closing.textContent()).includes(result.brief),
          'Trace omitted the qualified native closing reply',
        );
        assert.ok(
          previews.length &&
            previews.every(
              (url) =>
                url.searchParams.get('machine') &&
                url.searchParams.get('scope') === 'retained_history' &&
                /^[a-f0-9]{64}$/.test(url.searchParams.get('generation')),
            ),
        );
        assert.equal(await overflow(page), 0);
        assert.deepEqual(retired, []);
        assert.deepEqual(page.errors, []);
      } finally {
        await closePage(page);
      }
    }
  console.log('Native Trace preview typography and qualified text passed');
  return true;
}

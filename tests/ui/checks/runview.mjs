// Run view: exact spawn/return records, peer relays, bounded fan-out, recorded waits and transcript navigation.
import { served, data, goto, settled, reporter, ENV } from '../lib.mjs';
import path from 'node:path';
export default async function runView(browser) {
  const D = await data(),
    r = reporter('runview'),
    results = {};
  const base = structuredClone(D.model),
    root = base.turns.find((t) => t.sid === 'harbor'),
    start = root.at;
  root.sent = [];
  root.end = { st: 'done', why: 'replied', at: start + 600000 };
  const ids = [];
  for (let i = 0; i < 14; i++) {
    const sid = 'run-view-' + i,
      hid = 'run-view-h-' + i;
    ids.push(sid);
    root.sent.push(hid);
    const relay = i === 13;
    base.sessions[sid] = {
      ...structuredClone(base.sessions.harbor),
      name: 'Run agent ' + i,
      kind: relay ? 'Relayed' : 'Subagent',
      parent: relay ? null : 'harbor',
      lane: relay,
      state: 'done',
      start: start + i * 1000,
      last: start + 60000 + i * 1000,
      busy: [[start + i * 1000, start + 60000 + i * 1000]],
      wait_edges: [],
    };
    base.handoffs.push({
      id: hid,
      kind: relay ? 'relay' : 'spawn',
      from: 'harbor',
      to: sid,
      at: start + i * 1000,
      ...(i !== 12 ? { done: start + 60000 + i * 1000 } : {}),
      status: 'done',
      brief: 'Review the run',
      result: 'Reviewed',
    });
    base.turns.push({
      id: 'run-view-t-' + i,
      sid,
      at: start + i * 1000,
      start: hid,
      u: false,
      sent: [],
      end: { st: 'done', why: 'returned' },
    });
  }
  base.sessions.harbor.wait_edges = [
    {
      call: 'run-view-wait',
      tool: 'wait',
      targets: [ids[0]],
      start: start + 1000,
      end: start + 60000,
      turn: root.id,
    },
  ];
  base.version += '-run-view';
  for (const size of ['phone', 'desktop'])
    for (const dark of [false, true]) {
      const tag = size + (dark ? '-dark' : '-light'),
        page = await served(browser, { size, dark });
      await page.route(/\/api\/model(\?|$)/, (route) => route.fulfill({ json: base }));
      await page.route(/\/api\/tx\?/, (route) => {
        const sid = new URL(route.request().url()).searchParams.get('sid');
        if (sid === 'harbor')
          return route.fulfill({
            json: {
              sid,
              from: 0,
              to: 2,
              total: 2,
              calls: 0,
              errors: 0,
              entries: [
                { k: 'u', text: 'Run the agents', turn: root.id, slot: 0 },
                { k: 'h', id: 'run-view-h-0', slot: 1 },
              ],
            },
          });
        if (!ids.includes(sid)) return route.continue();
        return route.fulfill({
          json: {
            sid,
            from: 0,
            to: 1,
            total: 1,
            calls: 0,
            errors: 0,
            entries: [{ k: 'a', text: 'Reviewed', turn: 'run-view-t-0', slot: 0 }],
          },
        });
      });
      await page.reload();
      await settled(page);
      await goto(page, { v: 'trace', sid: root.sid, turn: root.id }, D);
      await page.waitForSelector('.agents-panel .agent-row');
      r.expect(
        (await page.locator('.agent-more').count()) === 1,
        tag + ': large fan-out has one explicit expand control',
      );
      const summary = await page.locator('.agents-summary').textContent();
      r.expect(
        summary.includes('15 agents') &&
          summary.includes('Recorded spawn overlap: 12') &&
          summary.includes('1 spawn end not recorded'),
        tag + ': participant count and completed recorded spawn overlap stay distinct: ' + summary,
      );
      r.expect(
        (await page.locator('.agent-row[data-agent-id="' + ids[13] + '"]').count()) === 1 &&
          (await page.locator('.agent-relayed-heading').textContent()) === 'Relayed to',
        tag + ': relay remains a peer',
      );
      r.expect(
        (await page.locator('.agent-wait').count()) === 1 &&
          (await page
            .locator('.agent-row[data-agent-id="' + ids[0] + '"]')
            .getAttribute('data-critical')) === 'true',
        tag + ': only recorded target waits are marked',
      );
      r.expect(
        (await page
          .locator('.agent-row[data-agent-id="' + ids[1] + '"]')
          .getAttribute('data-critical')) === 'false',
        tag + ': spawning alone does not imply waiting',
      );
      await page.locator('.agent-more').click();
      r.expect(
        (await page.locator('.agent-row[data-agent-id]').count()) === 15,
        tag + ': all 15 participating agents can be opened',
      );
      if (size === 'phone')
        r.expect(
          await page
            .locator('.agent-row')
            .evaluateAll((rows) => rows.every((n) => n.getBoundingClientRect().height >= 44)),
          tag + ': agent rows have 44px targets',
        );
      const dimensions = await page.evaluate(() => ({
        sideways: document.documentElement.scrollWidth - innerWidth,
        badBars: [...document.querySelectorAll('.agent-segment')].filter(
          (b) =>
            !Number.isFinite(parseFloat(getComputedStyle(b).left)) ||
            !Number.isFinite(parseFloat(getComputedStyle(b).width)),
        ).length,
      }));
      r.expect(
        dimensions.sideways <= 0 && dimensions.badBars === 0,
        tag + ': shared axis fits the viewport with finite positions',
      );
      await page.screenshot({ path: path.join(ENV.out, 'runview-' + tag + '.png') });
      if (size === 'desktop') {
        const missingReturn = await page
          .locator('.agent-edge-arrow[data-child="' + ids[12] + '"]')
          .count();
        r.expect(missingReturn === 0, tag + ': a missing return record never draws a return arrow');
      }
      await page.locator('.agent-row[data-agent-id="' + ids[0] + '"]').click();
      await page.waitForFunction(
        (id) => history.state?.v === 'session' && history.state?.id === id,
        ids[0],
      );
      await goto(page, { v: 'session', id: 'harbor' }, D);
      await page.locator('.child-card .cc-run').click();
      await page.waitForSelector('.agents-panel');
      r.expect(
        await page.evaluate(
          (id) => history.state.v === 'trace' && history.state.turn === id,
          root.id,
        ),
        tag + ": parent's spawn card links back to this run view",
      );
      r.expect(page.errors.length === 0, tag + ': page errors: ' + page.errors.join(' | '));
      const invalid = structuredClone(base);
      for (const [i, delta] of [
        [0, 0],
        [1, -1000],
      ]) {
        const h = invalid.handoffs.find((h) => h.id === 'run-view-h-' + i);
        h.done = h.at + delta;
      }
      invalid.version += '-invalid-bounds';
      await page.route(/\/api\/model(\?|$)/, (route) => route.fulfill({ json: invalid }));
      await page.reload();
      await settled(page);
      await goto(page, { v: 'trace', sid: root.sid, turn: root.id }, D);
      r.expect(
        (await page.locator('.agents-summary').textContent()).includes(
          'Recorded spawn overlap: 10',
        ),
        tag + ': zero/reversed spawn intervals cannot inflate or invert recorded overlap',
      );
      results[tag] = dimensions;
      await page.context().close();
    }
  // Real fixtures reproduce near-end tick collisions: the 52-minute ledger and 400-minute Sentinel runs.
  for (const dark of [false, true]) {
    const page = await served(browser, { size: 'phone', dark });
    const atlas = D.turns.find((t) => t.sid === 'atlas' && t.sent.length);
    await goto(page, { v: 'trace', sid: 'atlas', turn: atlas.id }, D);
    const summary = await page.locator('.agents-summary').textContent();
    r.expect(
      summary.includes('1 agent') && !summary.includes('overlap') && !summary.includes('at once'),
      'a root without spawns does not claim zero agent concurrency: ' + summary,
    );
    for (const sid of ['ledger', 'sentinel']) {
      const turn = D.turns.find((t) => t.sid === sid && t.sent.length);
      await goto(page, { v: 'trace', sid, turn: turn.id }, D);
      for (const width of [390, 320, 1280]) {
        await page.setViewportSize({ width, height: 844 });
        await page.evaluate(() => document.fonts.ready);
        await page.evaluate(
          () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))),
        );
        const axis = await page.evaluate(() => {
          const bounds = document.querySelector('.agent-axis-track').getBoundingClientRect();
          const labels = [...document.querySelectorAll('.agent-axis-tick > span')]
            .filter((n) => !n.hidden)
            .map((n) => {
              const b = n.getBoundingClientRect();
              return { text: n.textContent, left: b.left, right: b.right };
            });
          return { left: bounds.left, right: bounds.right, labels };
        });
        r.expect(
          axis.labels.length > 0 &&
            axis.labels.every(
              (n, i) =>
                n.left >= axis.left &&
                n.right <= axis.right + 0.1 &&
                (!i || n.left - axis.labels[i - 1].right >= 7.9),
            ),
          sid +
            ': ' +
            width +
            'px ' +
            (dark ? 'dark' : 'light') +
            ' axis labels fit without overlap after resize: ' +
            JSON.stringify(axis),
        );
      }
    }
    r.expect(page.errors.length === 0, 'axis resize page errors: ' + page.errors.join(' | '));
    await page.context().close();
  }
  r.results = results;
  return r.done();
}

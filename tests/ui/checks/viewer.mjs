// Ported from the mockup's viewer.js. The original had two independent halves: a phone-dark half that drove
// test-real.html (real logs) — a census of every step's cut/uncut preview against whether "View all" shows, then
// opened the longest cut call's "View all" sheet and checked its geometry, its full text, and that back/close/Escape
// each restore the page exactly — and a desktop-light half that already drove test7.html (the sample mockup) to
// check the same dialog on a wide screen and that a backdrop click closes it.
//
// Ported: the phone half now drives the served sample fixture instead of test-real.html (the same measurements, on
// the fixture's data); the desktop half already used the sample, so it only changes how the page is reached. Session
// ids are unchanged from the sample. Neither half named a sample handoff or turn id, so there is no id mapping here.
// The sidebar now shows eight top-level sessions and nested children, so the census visits each served session by
// route instead of treating every tree row as a visible, flat sidebar item. The phone half opens whichever session
// the census found a cut preview on (discovered at runtime, never hardcoded to "harbor"); the desktop half opens
// 'harbor' by session id, which is stable and does have a cut preview on its first step.
//
// Tool entries carry `src` and may carry `more` in the served viewer: "View all" shows whenever the server cut a
// preview, even where the preview isn't visually clipped, and the sheet fetches the full text from /api/entry before
// it opens (see gaps.json: previews are short in this fixture except harbor's first Bash output).
//
// Assertions:
//  - no page errors, phone or desktop.
//  - the phone census: mismatch === 0 (every cut preview shows "View all" and vice versa, for every step on every lane).
//  - the "View all" sheet: it opens (open === true), sits inside the viewport (sideways === 0), and its fetched full
//    text for every <pre> is at least as long as what the (possibly cut) preview showed — i.e. "View all" actually
//    fetched more, never less.
//  - back, the close button and Escape each close the sheet, restore the same history state, and leave the page's
//    expanded steps as they were (afterBack/afterClose/afterEsc: dialog === false, sameState === true, stepsStillOpen
//    unchanged, and back also drops the viewer-open html class).
//  - the measure at 1280, light and dark: an assistant message is at most 68ch wide (in its own font) with wide mode off, and
//    with it on is wider than 68ch and fills its column.
//  - the desktop dialog: at least one "View all" is visible on the sample's first expanded step, the dialog is not
//    sideways-clipped off the 1280px viewport, and a backdrop click closes it (closedByBackdrop === true).
//  - background commands (synthetic /api/tx pages and the bgcmd fixture), at 390 and 1280: each background step is one line
//    whose outcome ("running 8m 18s", "exit 0 · 1m 0s", "failed · 2m 0s") shows whole (scrollWidth <= clientWidth); at 390
//    the word "background" gives way to a marker named "background" before the verb, at 1280 the word stays; a summary
//    with finished background commands shows its counts ("(1 failed, 1 stopped)") inside its box on both.
//  - find counts a background command once: its finish row shows beside its step but is not a match of its own.
//  - a live update of a transcript with a running background call on an earlier turn fetches only the new tail (one
//    after= request, from the end), and the call still flips to its outcome when its finish row arrives, or to "no end
//    recorded" when the page says it no longer runs.
import path from 'node:path';
import { ENV, served, data, goto, reporter, wide, overflow } from '../lib.mjs';

// The shown background steps' layout: the outcome as drawn (innerText, so the hidden word drops out), whether it shows whole
// and on one line with the verb and command, and the marker and word that stand for "background".
const backgroundRows = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll('.step.background')]
      .filter((n) => n.getClientRects().length)
      .map((n) => {
        const b = n.querySelector(':scope > button'),
          sv = b.querySelector('.sv'),
          sa = b.querySelector('.sa'),
          sd = b.querySelector('.sd'),
          mark = b.querySelector('.bgmark');
        const rect = (x) => x.getBoundingClientRect(),
          mid = (x) => rect(x).top + rect(x).height / 2,
          line = (x) => rect(x).height < 2.2 * parseFloat(getComputedStyle(x).fontSize);
        return {
          tid: n.dataset.tid,
          shown: sd.innerText.replace(/\s+/g, ' ').trim(),
          whole:
            sd.scrollWidth <= sd.clientWidth &&
            rect(sd).right <= document.documentElement.clientWidth + 0.5,
          // A titled step has no verb (.sv), so only what is there is checked.
          oneLine: [sv, sa, sd]
            .filter(Boolean)
            .every((x) => line(x) && Math.abs(mid(x) - mid(sd)) < 4),
          mark:
            !!mark &&
            getComputedStyle(mark).display !== 'none' &&
            mark.getAttribute('role') === 'img' &&
            mark.getAttribute('aria-label') === 'background',
          word: getComputedStyle(sd.querySelector('.bgw')).display !== 'none',
        };
      }),
  );
const expectRows = (r, label, size, rows, outcomes) => {
  const phone = size === 'phone';
  r.expect(
    rows.length === outcomes.length &&
      rows.every(
        (x, i) =>
          x.shown === (phone ? '' : 'background · ') + outcomes[i] &&
          x.whole &&
          x.oneLine &&
          x.mark === phone &&
          x.word === !phone,
      ),
    label +
      ': each background step shows its whole outcome on one line, ' +
      (phone ? 'with the marker in place of the word' : 'after the word') +
      ': ' +
      JSON.stringify(rows),
  );
};
// A summary with finished background commands: its counts are inside its box (not clipped), and whether it is clamped.
const backgroundSummaries = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll('.tsum .tt.bgsum')]
      .filter((n) => n.getClientRects().length)
      .map((tt) => {
        const box = tt.getBoundingClientRect(),
          counts = tt.querySelector('.tt-counts');
        const inside =
          !!counts &&
          [...counts.getClientRects()].every(
            (q) =>
              q.left >= box.left - 0.5 &&
              q.right <= box.right + 0.5 &&
              q.top >= box.top - 0.5 &&
              q.bottom <= box.bottom + 0.5,
          );
        return {
          shown: tt.innerText.replace(/\s+/g, ' ').trim(),
          counts: counts?.textContent.trim() ?? null,
          inside,
          clamped: tt.scrollHeight > tt.clientHeight + 1,
        };
      }),
  );

// Synthetic /api/tx pages cover background lifecycles without reading another transcript fixture.
async function backgroundCommands(browser, r) {
  const results = {};
  for (const size of ['phone', 'desktop']) {
    const page = await served(browser, { size, path: '/s/claude/harbor' });
    try {
      const normal = (text) => text.replace(/\s+/g, ' ').trim();
      const tool = (tid, bg) => ({
        k: 'tool',
        name: 'Bash',
        arg: 'git fetch ' + tid,
        tid,
        ok: true,
        secs: '0.5s',
        out: 'Command running in background with ID: ' + tid,
        bg,
      });
      const foreground = {
        k: 'tool',
        name: 'Bash',
        arg: 'git status',
        ok: true,
        secs: '0.2s',
        out: 'Clean',
      };
      const read = {
        k: 'tool',
        name: 'Read',
        arg: 'README.md',
        ok: true,
        secs: '0.1s',
        out: 'Read',
      };
      let entries = [
        tool('running', { state: 'running', secs: '8m 18s', since: ENV.now - 498000 }),
        foreground,
        read,
      ];
      await page.route(/\/api\/tx\?/, async (route) => {
        const response = await route.fetch(),
          p = await response.json();
        if (p.sid !== 'harbor') {
          await route.fulfill({ response });
          return;
        }
        const rows = [
          { k: 'u', text: 'Background command coverage', turn: p.entries[0]?.turn },
          ...entries,
        ];
        await route.fulfill({
          response,
          json: { ...p, from: 0, to: rows.length, total: rows.length, entries: rows },
        });
      });
      const reload = async () => {
        await page.reload({ waitUntil: 'load' });
        await page.waitForFunction(() => !!document.querySelector('.step[data-tid]'));
        await page.evaluate(() =>
          document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((n) => n.click()),
        );
      };
      await reload();
      const running = await page.evaluate(() => {
        const step = document.querySelector('.step[data-tid="running"]');
        return {
          sd: step.querySelector('.sd').textContent,
          spin: step.querySelectorAll(':scope > button > .spin').length,
          summary: step.closest('.tgroup').querySelector('.tsum').textContent,
        };
      });
      r.expect(
        normal(running.sd) === 'background · running 8m 18s' && running.spin === 1,
        size + ': running background label and spinner: ' + JSON.stringify(running),
      );
      r.expect(
        normal(running.summary).includes('Ran 2 commands, read 1 file') &&
          normal(running.summary).includes('· running 8m 18s'),
        size + ': background command counts as running: ' + running.summary,
      );
      const runningRows = await backgroundRows(page);
      expectRows(r, size + ' running', size, runningRows, ['running 8m 18s']);
      await page.clock.setFixedTime(ENV.now + 2000);
      await page.waitForFunction(
        () =>
          document
            .querySelector('.step[data-tid="running"] .sd')
            ?.textContent.replace(/\s+/g, ' ')
            .trim() === 'background · running 8m 20s',
      );
      expectRows(r, size + ' ticked', size, await backgroundRows(page), ['running 8m 20s']);
      const ticked = await page.locator('.tsum .tl').textContent();
      r.expect(
        normal(ticked) === '· running 8m 20s',
        size + ': the group ticker follows the background call: ' + ticked,
      );
      await page.clock.setFixedTime(ENV.now);
      const summary = 'Background command "<script>literal</script>" completed (exit code 0)';
      entries = [
        tool('done:1', { state: 'done', exit: 0, secs: '12.0s', summary }),
        tool('failed', { state: 'failed', exit: 2, secs: '3.0s', summary: 'Failed (exit code 2)' }),
        tool('killed', { state: 'killed', secs: '4.0s' }),
        tool('unknown', { state: 'unknown' }),
        tool('done-no-exit', { state: 'done', secs: '5.0s' }),
        foreground,
        read,
        {
          k: 'bgend',
          call: 'done:1',
          state: 'done',
          exit: 0,
          label: 'Fetch changes ' + 'with a long description '.repeat(8),
        },
        { k: 'bgend', call: 'failed', state: 'failed', exit: 2, label: 'Fetch failed' },
        { k: 'bgend', call: 'killed', state: 'killed', label: 'Fetch stopped' },
        { k: 'bgend', call: 'done-no-exit', state: 'done', label: 'Fetch done' },
        { k: 'bgend', call: 'earlier-page', state: 'done', label: 'Earlier command' },
        { ...foreground, arg: 'git diff' },
      ];
      await reload();
      const ended = await page.evaluate(() => {
        const steps = [...document.querySelectorAll('.step[data-tid]')];
        return {
          steps: steps.map((n) => ({
            tid: n.dataset.tid,
            sd: n.querySelector('.sd').textContent,
            spin: n.querySelectorAll(':scope > button > .spin').length,
            err: n.classList.contains('err'),
          })),
          groups: document.querySelectorAll('.tgroup').length,
          summary: document.querySelector('.tsum .tt')?.textContent,
          ends: [...document.querySelectorAll('.bgend')].map((n) => ({
            text: n.textContent,
            button: !!n.querySelector('button'),
            err: n.classList.contains('err'),
          })),
        };
      });
      const expected = [
        'background · exit 0 · 12.0s',
        'background · failed · 3.0s',
        'background · stopped · 4.0s',
        'background · no end recorded',
        'background · done · 5.0s',
      ];
      r.expect(
        ended.steps.length === 5 &&
          ended.steps.every(
            (n, i) => normal(n.sd) === expected[i] && n.spin === 0 && n.err === (i === 1),
          ),
        size + ': terminal and unknown labels, errors and spinners: ' + JSON.stringify(ended.steps),
      );
      r.expect(
        ended.groups === 1,
        size + ': completion rows must stay inside one run: ' + ended.groups,
      );
      r.expect(
        normal(ended.summary) ===
          'Finished 5 background commands (1 failed, 1 stopped), ran 7 commands, read 1 file',
        size + ': completion counts and tool verbs: ' + ended.summary,
      );
      r.expect(
        ended.ends.length === 5 &&
          normal(ended.ends[0].text).startsWith('Background command completed · Fetch changes') &&
          normal(ended.ends[1].text) === 'Background command failed · Fetch failed' &&
          ended.ends[1].err &&
          normal(ended.ends[2].text) === 'Background command stopped · Fetch stopped',
        size + ': arrival rows: ' + JSON.stringify(ended.ends),
      );
      r.expect(
        ended.ends.slice(0, 4).every((n) => n.button) && ended.ends[4].button === false,
        size + ': only loaded originating calls get a jump button',
      );
      const endedRows = await backgroundRows(page);
      expectRows(r, size + ' ended', size, endedRows, [
        'exit 0 · 12.0s',
        'failed · 3.0s',
        'stopped · 4.0s',
        'no end recorded',
        'done · 5.0s',
      ]);
      const endedSummary = await backgroundSummaries(page);
      r.expect(
        endedSummary.length === 1 &&
          endedSummary[0].counts === '(1 failed, 1 stopped)' &&
          endedSummary[0].inside &&
          endedSummary[0].shown.startsWith(
            size === 'phone'
              ? '5 background (1 failed, 1 stopped), 7 commands'
              : 'Finished 5 background commands (1 failed, 1 stopped), ran 7 commands',
          ),
        size + ": the summary's counts show: " + JSON.stringify(endedSummary),
      );
      const done = page.locator('.step[data-tid="done:1"]');
      await done.locator(':scope > button').click();
      const finished = await done.locator('.out .io').allTextContents();
      r.expect(
        finished.at(-1) === 'Finished' &&
          (await done.locator('.out .finished').textContent()) === summary,
        size + ': expanded Finished summary is literal text',
      );
      r.expect(
        (await done.locator('script').count()) === 0,
        size + ': summary text must not create markup',
      );
      await page.locator('.bgend > button').first().click();
      r.expect(
        await done.evaluate((n) => n.classList.contains('flash')),
        size + ': the completion row highlights its originating call',
      );
      const over = await overflow(page);
      r.expect(over === 0, size + ': background rows must not scroll sideways: ' + over);
      r.expect(
        page.errors.length === 0,
        size + ': background page errors: ' + page.errors.join(' | '),
      );
      // Find counts a background command once: its step matches, and its finish row shows beside it without adding a match.
      let found = null;
      if (size === 'desktop') {
        await page.click('#find-btn');
        await page.keyboard.type('fetch failed', { delay: 10 });
        await page.waitForTimeout(200);
        found = await page.evaluate(() => ({
          count: document.querySelector('#topbar .fcount')?.textContent,
          steps: document.querySelectorAll('.turns .step[data-tid="failed"]').length,
          ends: document.querySelectorAll('.turns .bgend').length,
        }));
        r.expect(
          found.count === '1 match' && found.steps === 1 && found.ends === 1,
          size +
            ': find counts a background command and its finish row as one match: ' +
            JSON.stringify(found),
        );
        await page.click('#topbar [aria-label="Close find"]');
        await page.waitForTimeout(150);
      }
      // A finish in another run owns the failure count; a failed call without a loaded finish owns its own count.
      entries = [
        tool('late-failure', { state: 'failed', exit: 2, secs: '3.0s' }),
        foreground,
        { k: 'a', text: 'The command finished during the next run.' },
        { k: 'bgend', call: 'late-failure', state: 'failed', label: 'Late failure' },
        tool('no-finish-row', { state: 'failed', exit: 2, secs: '3.0s' }),
        foreground,
      ];
      await reload();
      const split = await page.evaluate(() =>
        [...document.querySelectorAll('.tsum')].map((n) => ({
          text: n.textContent.replace(/\s+/g, ' ').trim(),
          failure: n.querySelector('.tf')?.textContent.replace(/\s+/g, ' ').trim() ?? '',
        })),
      );
      r.expect(
        split.length === 2 &&
          split[0].failure === '' &&
          split[1].text.includes('Finished 1 background command (1 failed)') &&
          split[1].failure === '· 1 failed',
        size +
          ': each background failure is counted in its finish run, or its call run when no finish is loaded: ' +
          JSON.stringify(split),
      );
      await page.locator('.tsum').first().click(); // hide the originating step in the earlier run
      await page.locator('.bgend > button').click();
      r.expect(
        (await page.locator('.tsum').first().getAttribute('aria-expanded')) === 'true',
        size + ': jumping to a call opens its collapsed run',
      );
      // Load earlier: the initially plain completion row must become a jump button.
      await page.unroute(/\/api\/tx\?/);
      const prior = tool('paged-call', { state: 'done', exit: 0, secs: '1.0s' });
      await page.route(/\/api\/tx\?/, async (route) => {
        const response = await route.fetch(),
          p = await response.json();
        if (p.sid !== 'harbor') {
          await route.fulfill({ response });
          return;
        }
        const before = new URL(route.request().url()).searchParams.has('before');
        const rows = before
          ? [{ k: 'u', text: 'Earlier call', turn: p.entries[0]?.turn }, prior]
          : [
              {
                k: 'bgend',
                call: 'paged-call',
                state: 'done',
                label: 'Earlier call',
                turn: p.entries[0]?.turn,
              },
            ];
        await route.fulfill({
          response,
          json: { ...p, from: before ? 0 : 2, to: before ? 2 : 3, total: 3, entries: rows },
        });
      });
      await page.reload({ waitUntil: 'load' });
      await page.waitForFunction(() => !!document.querySelector('.bgend'));
      r.expect(
        (await page.locator('.bgend > button').count()) === 0,
        size + ': an unloaded originating call has a plain completion row',
      );
      await page.getByRole('button', { name: 'Load earlier', exact: true }).click();
      await page.waitForFunction(() => !!document.querySelector('.bgend > button'));
      r.expect(
        (await page.locator('.step[data-tid="paged-call"]').count()) === 1 &&
          (await page.locator('.bgend > button').count()) === 1,
        size + ': loading the earlier page enables the completion link',
      );
      r.expect(
        page.errors.length === 0,
        size + ': pagination background page errors: ' + page.errors.join(' | '),
      );
      results[size] = {
        running,
        ticked,
        ended,
        finished,
        overflow: over,
        split,
        runningRows,
        endedRows,
        endedSummary,
        found,
      };
    } finally {
      await page.context().close();
    }
  }
  return results;
}

// Screenshot a real synthetic Claude log at every size and theme, with the run and Finished output open.
async function backgroundScreenshots(browser, r) {
  const results = {};
  for (const [size, dark] of [
    ['phone', false],
    ['phone', true],
    ['desktop', false],
    ['desktop', true],
  ]) {
    const tag = (size === 'phone' ? '390' : '1280') + '-' + (dark ? 'dark' : 'light');
    const page = await served(browser, { extras: true, size, dark, path: '/s/claude/bgcmd' });
    try {
      await page.waitForFunction(() => document.querySelectorAll('.step[data-tid]').length === 4);
      await page.locator('.tsum').click();
      await page.locator('.step[data-tid="bg-completed"] > button').click();
      await page.locator('.tgroup').scrollIntoViewIfNeeded();
      await page.mouse.wheel(0, -1); // release the opening scroll pin before framing the run
      await page.evaluate(() => {
        const group = document.querySelector('.tgroup'),
          gap = document.querySelector('#topbar').getBoundingClientRect().bottom + 8;
        const sc = matchMedia('(max-width: 760px)').matches
          ? document.scrollingElement
          : document.querySelector('#main');
        sc.scrollTop += group.getBoundingClientRect().top - gap;
      });
      await page.waitForTimeout(200);
      const shown = await page.evaluate(() => {
        const normal = (text) => text.replace(/\s+/g, ' ').trim();
        return {
          labels: [...document.querySelectorAll('.step[data-tid] .sd')].map((n) =>
            normal(n.textContent),
          ),
          ends: [...document.querySelectorAll('.bgend')].map((n) => normal(n.textContent)),
          summary: normal(document.querySelector('.tsum').textContent),
          finished: document.querySelector('.step[data-tid="bg-completed"] .finished')?.textContent,
          open: document.querySelector('.tsum')?.getAttribute('aria-expanded') === 'true',
          visible: [
            ...document.querySelectorAll(
              '.step[data-tid], .bgend, .step[data-tid="bg-completed"] .finished',
            ),
          ].every((n) => n.getClientRects().length > 0),
        };
      });
      r.expect(
        shown.labels[0] === 'background · running 9m 0s' &&
          shown.labels[1] === 'background · exit 0 · 1m 0s' &&
          shown.labels[2] === 'background · failed · 2m 0s' &&
          shown.labels[3] === 'background · stopped · 3m 0s',
        tag + ': fixture covers running and all terminal states: ' + JSON.stringify(shown.labels),
      );
      r.expect(
        shown.ends.length === 3 &&
          shown.summary.includes(
            'Finished 3 background commands (1 failed, 1 stopped), ran 4 commands',
          ),
        tag + ': screenshot includes completion rows and summary: ' + JSON.stringify(shown),
      );
      r.expect(
        shown.open &&
          shown.visible &&
          shown.finished === 'Background command "cargo build" completed (exit code 0)',
        tag + ': run and Finished summary are expanded for the screenshot',
      );
      r.expect(
        (await page
          .locator('.step[data-tid="bg-completed"] > button')
          .getAttribute('aria-expanded')) === 'true' &&
          (await page.locator('.step[data-tid="bg-completed"] .finished').isVisible()),
        tag + ': completed command has its Finished summary open',
      );
      const rows = await backgroundRows(page);
      expectRows(r, tag, size, rows, [
        'running 9m 0s',
        'exit 0 · 1m 0s',
        'failed · 2m 0s',
        'stopped · 3m 0s',
      ]);
      const summary = await backgroundSummaries(page);
      r.expect(
        summary.length === 1 &&
          summary[0].counts === '(1 failed, 1 stopped)' &&
          summary[0].inside &&
          !summary[0].clamped &&
          summary[0].shown ===
            (size === 'phone'
              ? '3 background (1 failed, 1 stopped), 4 commands'
              : 'Finished 3 background commands (1 failed, 1 stopped), ran 4 commands'),
        tag + ': the summary shows its counts, unclamped: ' + JSON.stringify(summary),
      );
      const over = await overflow(page);
      r.expect(
        over === 0 && page.errors.length === 0,
        tag +
          ': screenshot fixture has no overflow or page errors: ' +
          JSON.stringify({ over, errors: page.errors }),
      );
      await page.screenshot({ path: path.join(ENV.out, 'bgcmd-' + tag + '.png') });
      results[tag] = { ...shown, rows, summary };
    } finally {
      await page.context().close();
    }
  }
  return results;
}

// A live update with a running background call on an earlier turn: only the new tail is fetched (one after= request, from the
// end), and the call flips to its outcome from the finish row, or to "no end recorded" when the page no longer lists it running.
async function backgroundTail(browser, r) {
  const results = {};
  for (const size of ['phone', 'desktop']) {
    const page = await served(browser, { size, path: '/s/claude/harbor' });
    try {
      const normal = (text) => text.replace(/\s+/g, ' ').trim();
      const turns = await page.evaluate(() =>
        fetch('/api/model?since=', { credentials: 'same-origin' })
          .then((x) => x.json())
          .then((m) => m.turns.filter((t) => t.sid === 'harbor').map((t) => t.id)),
      );
      // The fixture's harbor has one turn: a second, later one is added to the served model (as bar.mjs's bare turns are), so
      // the running calls and the tail that ends them are in different turns.
      r.expect(
        turns.length >= 1,
        size + ': harbor has a turn to start the tail test in: ' + turns.length,
      );
      const later = 'bt-tail-later';
      const since = ENV.now - 498000,
        launched = (tid, arg) => ({
          k: 'tool',
          name: 'Bash',
          arg,
          tid,
          ok: true,
          secs: '0.3s',
          out: 'Command running in background with ID: ' + tid,
          bg: { state: 'running', secs: '8m 18s', since },
        });
      const rows = [
        { k: 'u', text: 'Start the dev server and watch the checks', turn: turns[0] },
        launched('early-dev', 'npm run dev'),
        launched('early-watch', 'gh pr checks --watch'),
        { k: 'a', text: 'Both are running in the background.' },
        { k: 'u', text: 'Carry on', turn: later },
        { k: 'tool', name: 'Read', arg: 'README.md', ok: true, secs: '0.1s', out: 'Read' },
        { k: 'a', text: 'Read it.' },
      ].map((e, slot) => ({ ...e, slot }));
      const finish = {
        k: 'bgend',
        call: 'early-dev',
        state: 'done',
        exit: 0,
        label: 'Dev server',
        slot: rows.length,
        bg: {
          state: 'done',
          status: 'completed',
          exit: 0,
          secs: '9m 0s',
          summary: 'Background command "npm run dev" completed (exit code 0)',
        },
      };
      let ended = false,
        resumed = false,
        paused = false,
        late = false,
        bump = null;
      const afters = [];
      await page.route(/\/api\/tx\?/, async (route) => {
        const url = new URL(route.request().url()),
          q = url.searchParams;
        if (q.get('sid') !== 'harbor' || q.has('errors')) {
          await route.continue();
          return;
        }
        const lateFinish = {
          ...finish,
          call: 'early-watch',
          slot: rows.length + 1,
          label: 'Checks',
          bg: { state: 'failed', exit: 1, secs: '10m 0s' },
        };
        const all = late ? [...rows, finish, lateFinish] : ended ? [...rows, finish] : rows,
          running =
            resumed && !paused && !late
              ? ['early-watch']
              : ended
                ? []
                : ['early-dev', 'early-watch'];
        if (q.has('after')) afters.push(q.get('after'));
        const from = q.has('after') ? Math.min(Number(q.get('after')), all.length) : 0;
        await route.fulfill({
          json: {
            sid: 'harbor',
            from,
            to: all.length,
            total: all.length,
            calls: 5,
            errors: 0,
            entries: all.slice(from),
            bg_running: running,
          },
        });
      });
      // A moved mark for harbor, as when its log grows: the next poll's update fetches its tail.
      // The page's first model is /api/model with no query; later polls add ?since=. Both get the added turn.
      await page.route(/\/api\/model(\?|$)/, async (route) => {
        const url = new URL(route.request().url());
        if (bump) url.searchParams.set('since', '');
        const response = await route.fetch({ url: url.toString() });
        if (response.status() !== 200) {
          await route.fulfill({ response });
          return;
        } // a 304 has no body
        const m = await response.json(),
          base = m.turns.filter((t) => t.sid === 'harbor').at(-1);
        m.turns.push({
          id: later,
          sid: 'harbor',
          at: base?.at ?? 0,
          start: null,
          u: false,
          text: '',
          sent: [],
          end: null,
        });
        if (bump) {
          const [a, b, ...rest] = String(m.tx.harbor).split('.');
          m.version = bump;
          m.tx = {
            ...m.tx,
            harbor: [
              Number(a) + (late ? 4 : paused ? 3 : resumed ? 2 : 1),
              Number(b) + (late ? 4 : paused ? 3 : resumed ? 2 : 1),
              ...rest,
            ].join('.'),
          };
        }
        await route.fulfill({ response, json: m });
      });
      await page.reload({ waitUntil: 'load' });
      await page.waitForFunction(() => document.querySelectorAll('.step[data-tid]').length === 2);
      r.expect(
        (await page.locator('section.turn[data-turn="' + later + '"]').count()) === 1,
        size + ': the added later turn is drawn',
      );
      const sd = (tid) =>
        page.evaluate(
          (tid) =>
            document
              .querySelector('.step[data-tid="' + tid + '"] .sd')
              ?.textContent.replace(/\s+/g, ' ')
              .trim(),
          tid,
        );
      const before = [await sd('early-dev'), await sd('early-watch')];
      r.expect(
        before.every((t) => t === 'background · running 8m 18s'),
        size + ': both calls run before the update: ' + JSON.stringify(before),
      );
      ended = true;
      bump = 'bgtail-' + size;
      await page.waitForFunction(
        () =>
          document
            .querySelector('.step[data-tid="early-dev"] .sd')
            ?.textContent.replace(/\s+/g, ' ')
            .trim() === 'background · exit 0 · 9m 0s',
        null,
        { timeout: 15000 },
      );
      await page.waitForTimeout(300);
      await page.evaluate(() => {
        document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((b) => b.click());
        document
          .querySelector('.step[data-tid="early-dev"] > button[aria-expanded="false"]')
          ?.click();
      });
      const after = await page.evaluate(() => ({
        watch: document
          .querySelector('.step[data-tid="early-watch"] .sd')
          ?.textContent.replace(/\s+/g, ' ')
          .trim(),
        spins: [...document.querySelectorAll('.step[data-tid] > button > .spin')].length,
        finished:
          document.querySelector('.step[data-tid="early-dev"] .finished')?.textContent ?? null,
        callTurn:
          document.querySelector('.step[data-tid="early-dev"]')?.closest('section.turn')?.dataset
            .turn ?? null,
        endTurn: document.querySelector('.bgend')?.closest('section.turn')?.dataset.turn ?? null,
        ends: [...document.querySelectorAll('.bgend')].map((n) => ({
          text: n.textContent.replace(/\s+/g, ' ').trim(),
          button: !!n.querySelector('button'),
        })),
      }));
      r.expect(
        after.watch === 'background · no end recorded' && after.spins === 0,
        size +
          ': a call no longer listed as running stops, with no spinner left: ' +
          JSON.stringify(after),
      );
      r.expect(
        after.finished === finish.bg.summary &&
          after.ends.length === 1 &&
          after.ends[0].button &&
          normal(after.ends[0].text) === 'Background command completed · Dev server',
        size + ': the finish row arrives and links its call: ' + JSON.stringify(after),
      );
      r.expect(
        after.callTurn === turns[0] && after.endTurn === later,
        size +
          ": the finish row is drawn in the later turn, after the running calls' turn: " +
          JSON.stringify(after),
      );
      r.expect(
        afters.length === 1 && afters[0] === String(rows.length),
        size +
          ': the update fetches only the new tail, from the end (after=' +
          rows.length +
          '), not from the running call: ' +
          JSON.stringify(afters),
      );
      resumed = true;
      bump = 'bg-resumed-' + size;
      await page.waitForFunction(
        () =>
          document
            .querySelector('.step[data-tid="early-watch"]')
            ?.classList.contains('background-running'),
        null,
        { timeout: 15000 },
      );
      r.expect(
        (await sd('early-watch')) === 'background · running —',
        size + ': a previously unknown call resumes without a fabricated start time',
      );
      paused = true;
      bump = 'bg-paused-' + size;
      await page.waitForFunction(
        () =>
          document
            .querySelector('.step[data-tid="early-watch"] .sd')
            ?.textContent.includes('no end recorded'),
        null,
        { timeout: 15000 },
      );
      late = true;
      bump = 'bg-late-finish-' + size;
      await page.waitForFunction(
        () =>
          document
            .querySelector('.step[data-tid="early-watch"] .sd')
            ?.textContent.replace(/\s+/g, ' ')
            .includes('failed · 10m 0s'),
        null,
        { timeout: 15000 },
      );
      r.expect(
        (await sd('early-watch')) === 'background · failed · 10m 0s',
        size + ': a late finish patches a call before the tail cut',
      );
      r.expect(page.errors.length === 0, size + ': tail page errors: ' + page.errors.join(' | '));
      results[size] = { before, after, afters };
    } finally {
      await page.context().close();
    }
  }
  return results;
}

export default async function viewerCheck(browser) {
  const D = await data();
  const r = reporter('viewer');
  const R = {};

  // ---- Phone, dark: census on the sample lanes, then the "View all" sheet on the extras fixture's long output. ----
  {
    const page = await served(browser, { size: 'phone', dark: true, extras: true });
    const lanes = Object.keys(D.SESS);
    const C = { steps: 0, cut: 0, viewAllShown: 0, mismatch: 0 };
    const cutLanes = [];
    for (const id of lanes) {
      await goto(page, { v: 'session', id }, D);
      await page.waitForTimeout(150);
      await page.evaluate(() => {
        for (let k = 0; k < 3; k++)
          document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click());
        document
          .querySelectorAll('.step > button[aria-expanded="false"]')
          .forEach((x) => x.click());
      });
      // ResizeObserver measures previews after their containing turn opens; inspect the resulting state on a later frame.
      await page.waitForTimeout(100);
      const rr = await page.evaluate(() => {
        const r = { steps: 0, cut: 0, viewAllShown: 0, mismatch: 0, bad: [] };
        document.querySelectorAll('.step > button[aria-expanded="true"]').forEach((x) => {
          r.steps++;
          const o = x.parentElement.querySelector('.out');
          const cut =
            /^Output\s*·\s*(first|last)\s+\d+/.test(
              [...o.querySelectorAll('.io')].at(-1)?.textContent ?? '',
            ) ||
            /^Cut short/.test(o.querySelector('.cutnote')?.textContent ?? '') ||
            o.querySelector('.viewall:not(.viewscript) span')?.textContent === 'View all';
          /* a bare "View all", with no line count, is offered where the server cut the preview */ const shown =
            !!o.querySelector('.viewall:not(.viewscript)');
          if (cut) r.cut++;
          if (shown) r.viewAllShown++;
          if (cut !== shown) {
            r.mismatch++;
            r.bad.push({
              arg: x.querySelector('.sa')?.textContent.slice(0, 60),
              cut,
              shown,
              ios: [...o.querySelectorAll('.io')].map((i) => i.textContent),
              all: o.querySelector('.viewall:not(.viewscript)')?.textContent ?? null,
              lines: [...o.querySelectorAll('pre')].map((p) => p.textContent.split('\n').length),
            });
          }
        });
        return r;
      });
      for (const k of ['steps', 'cut', 'viewAllShown', 'mismatch']) C[k] += rr[k];
      (C.bad ??= []).push(...rr.bad);
      if (rr.cut) cutLanes.push(id);
    }
    R.census = C;
    R.cutLanes = cutLanes;
    r.expect(C.cut > 0, 'the phone census found no measured cut preview in the extras fixture');

    // The extras fixture contains a long Harbor output, so the sheet check exercises fetched text that exceeds its preview.
    // The lane is still discovered at runtime, never hardcoded to "harbor".
    const targetId = cutLanes[0];
    r.expect(
      !!targetId,
      "no lane had a cut preview to open 'View all' on (census.cut=" + C.cut + ')',
    );
    if (targetId) {
      await goto(page, { v: 'session', id: targetId }, D);
      await page.waitForTimeout(200);
      await page.evaluate(() => {
        document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click());
        document
          .querySelectorAll('.step > button[aria-expanded="false"]')
          .forEach((x) => x.click());
      });
      const btn = page.locator('.viewall:visible').first();
      const btnCount = await btn.count();
      r.expect(
        btnCount > 0,
        "no 'View all' button visible on " + targetId + ' despite a cut preview in the census',
      );
      if (btnCount > 0) {
        await btn.scrollIntoViewIfNeeded();
        await page.evaluate(() => window.scrollBy(0, -250));
        await page.waitForTimeout(300);
        const hashBefore = await page.evaluate(() => JSON.stringify(history.state));
        await page.screenshot({ path: path.join(ENV.out, 'v-preview.png') });
        const expected = await btn.evaluate((x) =>
          [...x.parentElement.querySelectorAll('pre')].map((q) => q.textContent.length),
        );
        await btn.click();
        await page.waitForTimeout(350);
        R.sheet = await page.evaluate(() => {
          const d = document.querySelector('dialog.panel.full');
          const r = d.getBoundingClientRect();
          const vw = document.documentElement.clientWidth;
          let out = 0;
          d.querySelectorAll('*').forEach((e) => {
            const b = e.getBoundingClientRect();
            if (b.width && (b.right > vw + 0.5 || b.left < -0.5)) out++;
          });
          return {
            open: d.open,
            x: r.x,
            y: r.y,
            w: r.width,
            h: r.height,
            vw,
            vh: innerHeight,
            pres: [...d.querySelectorAll('pre')].map((q) => q.textContent.length),
            sideways: out,
            bodyScrolls:
              d.querySelector('.panel-b').scrollHeight > d.querySelector('.panel-b').clientHeight,
            state: history.state?.sheet ?? 0,
            focus: document.activeElement?.className,
          };
        });
        R.sheet.previewPres = expected;
        await page.screenshot({ path: path.join(ENV.out, 'v-sheet.png') });
        await page.evaluate(() => {
          const b = document.querySelector('dialog.panel.full .panel-b');
          b.scrollTop = b.scrollHeight;
        });
        await page.waitForTimeout(200);
        await page.screenshot({ path: path.join(ENV.out, 'v-sheet-end.png') });
        // Back closes it and keeps the page as it was (still expanded, same route).
        await page.goBack();
        await page.waitForTimeout(350);
        R.afterBack = await page.evaluate(
          (h) => ({
            dialog: !!document.querySelector('dialog.panel.full'),
            sameState: JSON.stringify(history.state) === h,
            stepsStillOpen: document.querySelectorAll('.step > button[aria-expanded="true"]')
              .length,
            htmlLock: document.documentElement.classList.contains('panel-open'),
          }),
          hashBefore,
        );
        // The close button, then Escape, each restore the same history entry.
        await page.locator('.viewall:visible').first().scrollIntoViewIfNeeded();
        await page.waitForTimeout(150);
        const closeBefore = await page.evaluate(() => JSON.stringify(history.state));
        await page.locator('.viewall:visible').first().click();
        await page.waitForTimeout(250);
        await page.click('dialog.panel.full .panel-h .ibtn');
        await page.waitForTimeout(350);
        R.afterClose = await page.evaluate(
          (h) => ({
            dialog: !!document.querySelector('dialog.panel.full'),
            sameState: JSON.stringify(history.state) === h,
            stepsStillOpen: document.querySelectorAll('.step > button[aria-expanded="true"]')
              .length,
          }),
          closeBefore,
        );
        await page.locator('.viewall:visible').first().scrollIntoViewIfNeeded();
        await page.waitForTimeout(150);
        const escBefore = await page.evaluate(() => JSON.stringify(history.state));
        await page.locator('.viewall:visible').first().click();
        await page.waitForTimeout(250);
        await page.keyboard.press('Escape');
        await page.waitForTimeout(350);
        R.afterEsc = await page.evaluate(
          (h) => ({
            dialog: !!document.querySelector('dialog.panel.full'),
            sameState: JSON.stringify(history.state) === h,
            stepsStillOpen: document.querySelectorAll('.step > button[aria-expanded="true"]')
              .length,
          }),
          escBefore,
        );
      }
    }
    R.phoneErrors = page.errors;
    await page.context().close();
  }

  // ---- Desktop, light: the dialog on the sample's harbor lane. ----
  {
    const page = await served(browser, { size: 'desktop', dark: false });
    await goto(page, { v: 'session', id: 'harbor' }, D);
    await page.waitForTimeout(200);
    await page.evaluate(() => {
      document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click());
    });
    await page.locator('.step > button').first().click();
    await page.waitForTimeout(200);
    R.sample = { viewAll: await page.locator('.viewall:visible').count() };
    r.expect(
      R.sample.viewAll > 0,
      "no View all button on the sample harbor lane's first expanded step",
    );
    if (R.sample.viewAll) {
      await page.locator('.viewall:visible').first().click();
      await page.waitForTimeout(300);
      R.desk = await page.evaluate(() => {
        const r = document.querySelector('dialog.panel.full').getBoundingClientRect();
        return {
          x: Math.round(r.x),
          w: Math.round(r.width),
          h: Math.round(r.height),
          vh: innerHeight,
          vw: document.documentElement.clientWidth,
        };
      });
      await page.screenshot({ path: path.join(ENV.out, 'v-desk.png') });
      await page.mouse.click(40, 40);
      await page.waitForTimeout(250);
      R.desk.closedByBackdrop = !(await page.$('dialog.panel.full'));
    }
    R.deskErrors = page.errors;
    await page.context().close();
  }

  // ---- Desktop, light and dark: an assistant message's measure with wide mode off (68ch) and on (the column's width). ----
  R.measure = {};
  for (const dark of [false, true]) {
    const page = await served(browser, { size: 'desktop', dark });
    const ids = ['harbor', ...Object.keys(D.SESS).filter((id) => id !== 'harbor')];
    let found = null;
    for (const id of ids) {
      await goto(page, { v: 'session', id }, D);
      await page.waitForTimeout(100);
      if (await page.locator("#page section[aria-label='Transcript'] .msg.assistant").count()) {
        found = id;
        break;
      }
    }
    // The message's width against a probe that is 68ch in the message's own font, and against the column it sits in.
    const measure = () =>
      page.evaluate(() => {
        const msg = document.querySelector("#page section[aria-label='Transcript'] .msg.assistant");
        const probe = document.createElement('div');
        probe.style.cssText = 'position:absolute;visibility:hidden;height:0;width:68ch';
        msg.append(probe);
        const ch68 = probe.getBoundingClientRect().width;
        probe.remove();
        const cs = getComputedStyle(msg.parentElement),
          col =
            msg.parentElement.getBoundingClientRect().width -
            parseFloat(cs.paddingLeft) -
            parseFloat(cs.paddingRight);
        return {
          wide: document.querySelector('#page').classList.contains('wide-mode'),
          w: msg.getBoundingClientRect().width,
          ch68,
          col,
          vw: document.documentElement.clientWidth,
          sideways: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        };
      });
    const key = dark ? 'dark' : 'light';
    R.measure[key] = { session: found };
    if (found) {
      R.measure[key].off = await measure();
      await wide(page, true);
      await page.waitForTimeout(150);
      R.measure[key].on = await measure();
      await page.screenshot({ path: path.join(ENV.out, 'wide-session-' + key + '.png') });
    }
    R.measure[key].errors = page.errors;
    await page.context().close();
  }

  R.background = await backgroundCommands(browser, r);
  R.backgroundScreenshots = await backgroundScreenshots(browser, r);
  R.backgroundTail = await backgroundTail(browser, r);
  r.results = R;
  r.expect(
    (R.phoneErrors ?? []).length === 0,
    'phone page errors: ' + (R.phoneErrors ?? []).join(' | '),
  );
  r.expect(
    (R.deskErrors ?? []).length === 0,
    'desktop page errors: ' + (R.deskErrors ?? []).join(' | '),
  );
  r.expect(
    R.census?.mismatch === 0,
    'census mismatch=' + R.census?.mismatch + ' ' + JSON.stringify(R.census?.bad ?? []),
  );
  r.expect(!!R.sheet, "the 'View all' sheet was never opened, so nothing below could be checked");
  if (R.sheet) {
    r.expect(R.sheet.open === true, 'sheet did not open');
    r.expect(R.sheet.sideways === 0, 'sheet sideways=' + R.sheet.sideways);
    r.expect(
      R.sheet.pres.every((len, i) => len >= (R.sheet.previewPres[i] ?? 0)),
      'View all fetched less than the preview showed',
    );
    // Here the preview is cut on screen only (the sample's output is under the server's 1536-byte preview), so the sheet
    // holds the same text, unclipped. That View all fetches a preview the server cut is asserted in extras.mjs.
  }
  r.expect(!!R.afterBack, 'back was never tried on the sheet');
  if (R.afterBack) {
    r.expect(
      R.afterBack.dialog === false && R.afterBack.sameState && R.afterBack.htmlLock === false,
      'back did not restore the page: ' + JSON.stringify(R.afterBack),
    );
    r.expect(R.afterBack.stepsStillOpen > 0, 'steps did not stay open after back');
  }
  r.expect(!!R.afterClose, 'the close button was never tried on the sheet');
  if (R.afterClose)
    r.expect(
      R.afterClose.dialog === false && R.afterClose.sameState,
      'close button did not restore the page: ' + JSON.stringify(R.afterClose),
    );
  r.expect(!!R.afterEsc, 'Escape was never tried on the sheet');
  if (R.afterEsc)
    r.expect(
      R.afterEsc.dialog === false && R.afterEsc.sameState,
      'Escape did not restore the page: ' + JSON.stringify(R.afterEsc),
    );
  r.expect(!!R.desk, 'the desktop dialog was never opened, so nothing below could be checked');
  if (R.desk) {
    r.expect(
      R.desk.x >= 0 && R.desk.x + R.desk.w <= R.desk.vw + 0.5,
      'desktop dialog sideways: ' + JSON.stringify(R.desk),
    );
    r.expect(R.desk.closedByBackdrop === true, 'backdrop click did not close the desktop dialog');
  }

  for (const key of ['light', 'dark']) {
    const m = R.measure[key];
    r.expect(
      !!m.session,
      'no session with an assistant message, so the wide-mode measure (' +
        key +
        ') was never checked',
    );
    r.expect(
      m.errors.length === 0,
      'wide-mode measure page errors (' + key + '): ' + m.errors.join(' | '),
    );
    if (!m.session) continue;
    r.expect(
      m.off.wide === false && m.on.wide === true,
      'the wide toggle did not switch the mode (' + key + '): ' + JSON.stringify(m),
    );
    r.expect(
      m.off.w <= m.off.ch68 + 0.5,
      'with wide mode off an assistant message is wider than 68ch (' +
        key +
        '): ' +
        JSON.stringify(m.off),
    );
    r.expect(
      m.on.w > m.on.ch68 + 1,
      'with wide mode on an assistant message is still capped at 68ch (' +
        key +
        '): ' +
        JSON.stringify(m.on),
    );
    r.expect(
      m.on.w >= m.on.col - 1 && m.on.w <= m.on.col + 1,
      'with wide mode on an assistant message does not fill its column (' +
        key +
        '): ' +
        JSON.stringify(m.on),
    );
    r.expect(
      m.on.sideways === 0,
      'wide mode scrolls sideways (' + key + '): ' + JSON.stringify(m.on),
    );
  }

  return r.done();
}

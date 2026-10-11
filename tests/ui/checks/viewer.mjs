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
//  - background commands (qualified selected ranges and the bgcmd fixture), at 390 and 1280: each background step is one line
//    whose outcome ("running 8m 18s", "exit 0 · 1m 0s", "failed · 2m 0s") shows whole (scrollWidth <= clientWidth); at 390
//    the word "background" gives way to a marker named "background" before the verb, at 1280 the word stays; a summary
//    with finished background commands shows its counts ("(1 failed, 1 stopped)") inside its box on both.
//  - find counts a background command once: its finish row shows beside its step but is not a match of its own.
//  - a live update of a transcript with a running background call on an earlier turn fetches only the new tail (one
//    after= request, from the end), and the call still flips to its outcome when its finish row arrives, or to "no end
//    recorded" when the page says it no longer runs.
import path from 'node:path';
import { selectedFixture } from '../selected-fixture.mjs';
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

// Qualified selected ranges cover background lifecycles without reading another transcript fixture.
async function backgroundCommands(browser, r) {
  const results = {};
  for (const size of ['phone', 'desktop']) {
    const page = await served(browser, { size });
    let current = true,
      phase = 'running',
      reads = 0;
    const retired = [];
    try {
      page.on('request', (request) => {
        if (/\/api\/(model|tx|events)(?:[?#/]|$)/.test(request.url())) retired.push(request.url());
      });
      await selectedFixture(page, 'harbor', (data) => {
        const base = data.entries.find((row) => row.k === 'tool'),
          owner = base.turn ?? data.entries.find((row) => row.turn)?.turn;
        data.session.details.live_observation = current;
        data.session.details.state = current ? 'work' : 'off';
        data.entries = [
          { ...base, k: 'u', text: 'Background command coverage', turn: owner },
          {
            ...base,
            k: 'tool',
            name: 'Bash',
            arg: 'git fetch origin',
            tid: 'native-background',
            live: current,
            ok: true,
            secs: '0.5s',
            turn: owner,
            out: 'Command running in background',
            bg:
              phase === 'running'
                ? { state: 'running', secs: '8m 18s', since: ENV.now - 498000 }
                : {
                    state: 'failed',
                    exit: 1,
                    secs: '2m 0s',
                    summary: '<script>literal</script> failed',
                  },
            more: undefined,
            field: undefined,
            clipped: false,
          },
        ];
        reads++;
      });
      const D = await data();
      await goto(page, { v: 'session', id: 'harbor' }, D);
      await page.waitForSelector('.step[data-tid="native-background"]');
      if (await page.locator('.tsum[aria-expanded="false"]').count())
        await page.locator('.tsum[aria-expanded="false"]').click();
      const running = await backgroundRows(page);
      r.expect(
        running.length === 1 && running[0].whole && running[0].oneLine,
        size + ': qualified background outcome fits its reading row',
      );
      await page.clock.setFixedTime(ENV.now + 2000);
      await page.waitForFunction(() =>
        document
          .querySelector('.step[data-tid="native-background"] .sd')
          ?.textContent.includes('8m 20s'),
      );
      const ticked = await backgroundRows(page);
      r.expect(ticked[0].shown.includes('8m 20s'), size + ': current qualified activity ticks');
      phase = 'done';
      const before = reads;
      await page.clock.setFixedTime(ENV.now + 12000);
      await page.waitForFunction(
        () =>
          document
            .querySelector('.step[data-tid="native-background"] .sd')
            ?.textContent.includes('failed'),
        null,
        { timeout: 15000 },
      );
      r.expect(reads > before, size + ': selected range revalidation updates the terminal outcome');
      await page.locator('.step[data-tid="native-background"] > button').click();
      r.expect(
        (await page.locator('.finished').textContent()).includes('<script>literal</script>') &&
          (await page.locator('.finished script').count()) === 0,
        size + ': background terminal text remains escaped',
      );
      current = false;
      phase = 'running';
      await page.reload();
      await page.waitForSelector('.step[data-tid="native-background"]');
      const frozen = await page.locator('.step[data-tid="native-background"] .sd').textContent();
      await page.clock.setFixedTime(ENV.now + 120000);
      await page.waitForTimeout(1200);
      r.expect(
        (await page.locator('.step[data-tid="native-background"] .sd').textContent()) === frozen,
        size + ': recorded history cannot gain a running clock',
      );
      r.expect(retired.length === 0 && page.errors.length === 0, size + ': no workspace reader');
      results[size] = { running, ticked, reads, frozen };
    } finally {
      await page.context().close();
    }
  }
  return results;
}
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
        shown.labels[0] === 'background · no end recorded' &&
          shown.labels[1] === 'background · exit 0 · 1m 0s' &&
          shown.labels[2] === 'background · failed · 2m 0s' &&
          shown.labels[3] === 'background · stopped · 3m 0s',
        tag +
          ': recorded fixture covers unknown and all terminal states: ' +
          JSON.stringify(shown.labels),
      );
      r.expect(
        shown.ends.length === 3 &&
          shown.summary.includes(
            'Finished 3 background commands (1 failed, 1 stopped), ran 1 command',
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
        'no end recorded',
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
            'Finished 3 background commands (1 failed, 1 stopped), ran 1 command',
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
// Native range growth and later-turn isolation are covered by viewer-backlog,
// catalog-served and the selected store tests; no workspace model is patched here.
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

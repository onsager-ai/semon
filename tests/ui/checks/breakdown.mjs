// Analytics' ranked lists: the Breakdown's control strip and group heading stay pinned under the top bar while its rows scroll, and
// every ranked row (the Breakdown's and the three top-session lists) keeps its figures inside the gutter, with a bar scaled to its value.
//
// The served fixture has a handful of repos, so the answer for /api/analytics is rewritten in the browser (only the browser's copy):
// 24 repos, 6 machines and 6 models in the Breakdown, the widest figures the layout has to hold ($12,345.67, 999.9 h), and six rows in each
// top-session list (the real sessions, so each row is a link to a session that exists). Shots of the result go to out/analytics/.
import fs from 'node:fs';
import path from 'node:path';
import { served, data, reporter, overflow, ENV } from '../lib.mjs';

const HOUR = 3600000,
  MIN = 60000;
const drawn = (page, query) =>
  page.waitForFunction(
    (q) => document.querySelector('.analytics-metrics[data-analytics-ready]')?.dataset.query === q,
    query,
  );

// The values the top lists show, in list order, so the bars can be held to them.
const TOP = {
  'Top sessions · busy time': {
    key: 'busy',
    values: [14 * HOUR + 24 * MIN, 9 * HOUR, 5 * HOUR, 2 * HOUR, HOUR, 30 * MIN],
    field: 'ms',
  },
  'Top sessions · waited on': {
    key: 'waited',
    values: [3 * HOUR, 2 * HOUR, HOUR, 40 * MIN, 12 * MIN, 3 * MIN],
    field: 'ms',
  },
  'Most expensive sessions · API-equivalent cost': {
    key: 'cost',
    values: [12345.67, 6000, 3000, 900, 203.27, 12],
    field: 'usd',
  },
};

function fakeAnswer(A, D) {
  const pad = (i) => String(i + 1).padStart(2, '0');
  A.breakdown.repo = Array.from({ length: 24 }, (_, i) => ({
    repo:
      i === 2
        ? 'a-very-long-repository-name-that-has-to-be-cut-short-in-its-column'
        : 'repo-' + pad(i),
    ms: i === 0 ? 999.9 * HOUR : i === 1 ? 58.4 * HOUR : 40 * HOUR * 0.88 ** i,
    usd: i === 0 ? 12345.67 : i === 1 ? 1118.01 : 500 * 0.85 ** i,
    unpriced_models: [],
    sessions: 30 - i,
  }));
  A.breakdown.machine = Array.from({ length: 6 }, (_, i) => ({
    machine: 'machine-' + pad(i),
    ms: 30 * HOUR * 0.7 ** i,
    usd: 800 * 0.7 ** i,
    unpriced_models: [],
    sessions: 9 - i,
  }));
  A.breakdown.model = Array.from({ length: 6 }, (_, i) => ({
    harness: i % 2 ? 'codex' : 'claude',
    model: 'model-' + pad(i),
    ms: 30 * HOUR * 0.7 ** i,
    usd: 800 * 0.7 ** i,
    unpriced_models: [],
    sessions: 9 - i,
  }));
  const ids = Object.keys(D.SESS);
  for (const spec of Object.values(TOP)) {
    A.top[spec.key] = spec.values.map((v, i) => ({
      sid: ids[i % ids.length],
      [spec.field]: v,
      ...(spec.field === 'usd' ? { unpriced_models: [] } : {}),
    }));
  }
  for (const id of ids) A.sessions[id] ??= { name: D.SESS[id].name, harness: D.SESS[id].harness };
  return A;
}

// Serves the fake answer for every /api/analytics request of the page, with a tag of its own so the page's kept answer is answered with a 304.
async function fake(page, D, state = { n: 0 }) {
  await page.route(
    (u) => u.pathname === '/api/analytics',
    async (route) => {
      const headers = { ...route.request().headers() };
      delete headers['if-none-match'];
      const res = await route.fetch({ headers }),
        tag = (res.headers().etag ?? '"a"') + '-fake' + state.n;
      if (route.request().headers()['if-none-match'] === tag)
        return route.fulfill({ status: 304, headers: { etag: tag } });
      return route.fulfill({
        status: 200,
        headers: { 'content-type': 'application/json', etag: tag },
        body: JSON.stringify(fakeAnswer(await res.json(), D)),
      });
    },
  );
}

async function open(browser, D, { size, dark = false }) {
  const page = await served(browser, { size, dark, path: '/analytics' });
  await drawn(page, 'range=7d');
  const state = (page.fakeState = { n: 0 });
  await fake(page, D, state);
  await page.click('#topbar .analytics-range button:has-text("24 h")');
  await drawn(page, 'range=24h');
  await page.waitForFunction(
    () => document.querySelectorAll('.analytics-row[data-breakdown="repo"]').length === 24,
  );
  // A redraw puts the page back where it was two frames later (restore()); scroll only once fonts are in and two frames have passed.
  await page.evaluate(() =>
    document.fonts.ready.then(
      () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
    ),
  );
  return page;
}

// Scrolls the nth row of a breakdown group to the top of the screen, so the list fills the view under whatever is pinned.
// Asked again while the row is not at the top, in case a redraw moved the page back.
const scrollToRow = async (page, group, n, r, mode) => {
  for (let tries = 0; tries < 4; tries++) {
    await page.evaluate(
      ([g, i]) =>
        document
          .querySelectorAll('.analytics-row[data-breakdown="' + g + '"]')
          [i].scrollIntoView({ block: 'start' }),
      [group, n],
    );
    await page.waitForTimeout(250);
    const at = await page.evaluate(
      ([g, i]) =>
        document
          .querySelectorAll('.analytics-row[data-breakdown="' + g + '"]')
          [i].getBoundingClientRect().top,
      [group, n],
    );
    if (Math.abs(at) < 3) return true;
  }
  r.expect(
    false,
    mode +
      ': the ' +
      group +
      ' row ' +
      n +
      ' could not be scrolled to the top of the screen in 4 tries',
  );
  return false;
};

// Where the bar, the control strip and each group heading are, and whether each is what a tap at its middle would reach.
const probe = (page) =>
  page.evaluate(() => {
    const box = (n) => {
      const r = n.getBoundingClientRect();
      return { top: r.top, bottom: r.bottom, left: r.left, right: r.right };
    };
    const reaches = (n, x, y) => {
      const e = document.elementFromPoint(x, y);
      return !!e && (e === n || n.contains(e));
    };
    const bar = document.querySelector('#topbar'),
      toggle = document.querySelector('.analytics-measure'),
      strip = toggle.closest('.analytics-bd-bar') ?? toggle,
      sb = box(strip),
      tb = box(toggle);
    const opaque = (n) => {
      const m = /rgba?\(([^)]+)\)/.exec(getComputedStyle(n).backgroundColor);
      if (!m) return false;
      const p = m[1]
        .split(/[ ,/]+/)
        .filter(Boolean)
        .map(Number);
      return (p[3] ?? 1) === 1;
    };
    return {
      scroll: document.scrollingElement.scrollTop + document.querySelector('#main').scrollTop,
      bar: box(bar),
      strip: sb,
      stripPosition: getComputedStyle(strip).position,
      stripOpaque: opaque(strip),
      toggle: tb,
      stripTopmost: reaches(strip, sb.left + 2, sb.top + 2),
      toggleTopmost: reaches(toggle, (tb.left + tb.right) / 2, (tb.top + tb.bottom) / 2),
      headings: [...document.querySelectorAll('.analytics-breakdowns .analytics-panel h3')].map(
        (h) => {
          const b = box(h);
          return {
            text: h.textContent,
            ...b,
            position: getComputedStyle(h).position,
            opaque: opaque(h),
            topmost: reaches(h, b.left + 8, (b.top + b.bottom) / 2),
          };
        },
      ),
    };
  });

export default async function breakdownCheck(browser) {
  const D = await data(),
    r = reporter('breakdown'),
    results = {},
    errors = [];
  const shots = path.join(ENV.out, 'analytics');
  fs.mkdirSync(shots, { recursive: true });

  for (const [mode, size] of [
    ['phone', 'phone'],
    ['desktop', 'desktop'],
  ]) {
    const page = await open(browser, D, { size }),
      stacked = size === 'phone',
      res = (results[mode] = {});

    // Pinned: with a group's rows in view, the strip sits directly under the bar, and the group's heading directly under the strip. At
    // 390 the groups are stacked and one heading is pinned at a time; at 1280 they sit side by side and all three are.
    const groups = [
      ['repo', 12, 'By repo'],
      ['machine', 2, 'By machine'],
      ['harness', 2, 'By harness and model'],
    ];
    res.pinned = {};
    for (const [group, index, heading] of groups) {
      await scrollToRow(page, group, index, r, mode);
      const p = (res.pinned[group] = await probe(page));
      r.expect(
        Math.abs(p.strip.top - p.bar.bottom) <= 1,
        mode +
          ': with ' +
          group +
          " rows in view, the control strip's top is not at the bar's bottom: " +
          JSON.stringify({ bar: p.bar, strip: p.strip }),
      );
      r.expect(
        p.toggle.top >= p.strip.top - 1 &&
          p.toggle.bottom <= p.strip.bottom + 1 &&
          p.stripPosition === 'sticky',
        mode +
          ': the toggle is not inside a sticky strip: ' +
          JSON.stringify({ strip: p.strip, toggle: p.toggle, position: p.stripPosition }),
      );
      r.expect(
        p.stripTopmost && p.toggleTopmost,
        mode +
          ': the strip or the toggle is covered by something else: ' +
          JSON.stringify({ strip: p.stripTopmost, toggle: p.toggleTopmost }),
      );
      r.expect(p.stripOpaque, mode + ": the strip's background is not opaque");
      const own = p.headings.find((h) => h.text === heading);
      for (const h of stacked ? [own] : p.headings) {
        r.expect(
          h &&
            Math.abs(h.top - p.strip.bottom) <= 1 &&
            h.topmost &&
            h.opaque &&
            h.position === 'sticky',
          mode +
            ': with ' +
            group +
            " rows in view, heading '" +
            (h?.text ?? heading) +
            "' is not pinned directly under the strip: " +
            JSON.stringify({ strip: p.strip, heading: h }),
        );
      }
      // The other headings of a stacked layout have left: the groups above are pushed out by their own end, the ones below have not arrived.
      if (stacked)
        for (const h of p.headings.filter((x) => x.text !== heading))
          r.expect(
            h.top > p.strip.bottom + 1 || h.bottom <= p.strip.bottom + 1,
            mode +
              ": heading '" +
              h.text +
              "' is drawn over the pinned one: " +
              JSON.stringify({ strip: p.strip, heading: h }),
          );
    }
    // A shot with the strip pinned over the repo rows and one over the machine rows.
    await scrollToRow(page, 'repo', 12, r, mode);
    await page.screenshot({
      path: path.join(shots, 'breakdown-' + (stacked ? '390' : '1280') + '-light-repo.png'),
    });
    await scrollToRow(page, 'machine', 2, r, mode);
    await page.screenshot({
      path: path.join(shots, 'breakdown-' + (stacked ? '390' : '1280') + '-light-machine.png'),
    });

    // Scrolled past the Breakdown, the strip and the headings have gone with it.
    await page.evaluate(() => {
      document.querySelector('#page').style.paddingBottom = '1600px';
      const sc = matchMedia('(max-width: 760px)').matches
        ? document.scrollingElement
        : document.querySelector('#main');
      sc.scrollTop = sc.scrollHeight;
    });
    await page.waitForTimeout(150);
    const past = (res.past = await probe(page));
    r.expect(
      past.strip.bottom <= past.bar.bottom + 1 &&
        past.headings.every((h) => h.bottom <= past.bar.bottom + 1),
      mode +
        ': past the Breakdown, the strip or a heading is still pinned: ' +
        JSON.stringify({ bar: past.bar, strip: past.strip, headings: past.headings }),
    );
    await page.evaluate(() => {
      document.querySelector('#page').style.paddingBottom = '';
    });

    // Figures: with the widest values in the first row ($12,345.67, 999.9 h), every figure of every Breakdown row is inside the row and the gutter, the
    // figures line up down the list, and the bar shrinks instead. The app writes money as $12345.67; the comma form is the wider of the two.
    await page.evaluate(() => {
      document.querySelector('.analytics-row[data-breakdown="repo"] .row-cost').textContent =
        '$12,345.67';
    });
    await page.evaluate(() =>
      document
        .querySelector('.analytics-row[data-breakdown="repo"]')
        .scrollIntoView({ block: 'start' }),
    );
    await page.waitForTimeout(100);
    const fit = (res.fit = await page.evaluate(() => {
      const vw = document.documentElement.clientWidth,
        text = (n) => {
          const q = document.createRange();
          q.selectNodeContents(n);
          const b = q.getBoundingClientRect();
          return { left: b.left, right: b.right };
        };
      const rows = [...document.querySelectorAll('.analytics-breakdowns .analytics-row')];
      return {
        vw,
        rows: rows.map((row) => {
          const box = row.getBoundingClientRect(),
            track = row.querySelector('.row-track').getBoundingClientRect();
          return {
            group: row.dataset.breakdown,
            title: row.querySelector('.row-title').textContent,
            right: box.right,
            track: track.width,
            figures: ['row-count', 'row-hours', 'row-cost'].map((c) => ({
              c,
              ...text(row.querySelector('.' + c)),
              align: getComputedStyle(row.querySelector('.' + c)).textAlign,
              nums: getComputedStyle(row.querySelector('.' + c)).fontVariantNumeric,
            })),
          };
        }),
      };
    }));
    r.expect(
      fit.rows.filter((row) => row.group === 'repo').length === 24,
      mode + ': expected 24 repo rows in the fake answer: ' + fit.rows.length,
    );
    for (const row of fit.rows) {
      for (const f of row.figures) {
        r.expect(
          f.right <= Math.min(row.right, fit.vw - 16) + 0.5,
          mode +
            ": '" +
            row.title +
            "' " +
            f.c +
            ' runs past the row or the 16px gutter (right ' +
            f.right +
            ', row ' +
            row.right +
            ', limit ' +
            (fit.vw - 16) +
            ')',
        );
        r.expect(
          f.align === 'right' && /tabular-nums/.test(f.nums),
          mode +
            ": '" +
            row.title +
            "' " +
            f.c +
            ' is not right-aligned with tabular numbers: ' +
            JSON.stringify(f),
        );
      }
      r.expect(
        row.track >= 40,
        mode + ": '" + row.title + "' bar is " + row.track + 'px wide, want at least 40',
      );
    }
    for (const group of ['repo', 'machine', 'harness']) {
      const rights = (c) =>
        fit.rows
          .filter((row) => row.group === group)
          .map((row) => row.figures.find((f) => f.c === c).right);
      for (const c of ['row-hours', 'row-cost'])
        r.expect(
          Math.max(...rights(c)) - Math.min(...rights(c)) <= 1,
          mode +
            ': the ' +
            group +
            ' ' +
            c +
            " figures don't line up down the list: " +
            JSON.stringify(rights(c)),
        );
    }
    if (stacked)
      r.expect(
        (await overflow(page)) === 0,
        mode + ': the page scrolls sideways or something is drawn past the edge',
      );

    // The top-session lists: a bar per row scaled to the list's largest value (within 2 %), in its harness's colour; figures inside the gutter; each row a link to its session.
    await page.evaluate(() =>
      document.querySelector('.analytics-split').scrollIntoView({ block: 'start' }),
    );
    await page.waitForTimeout(100);
    const lists = (res.lists = await page.evaluate(() => {
      const vw = document.documentElement.clientWidth,
        text = (n) => {
          const q = document.createRange();
          q.selectNodeContents(n);
          return q.getBoundingClientRect().right;
        };
      const token = (h) => {
        const p = document.createElement('i');
        p.style.background = 'var(--h-' + h + ')';
        document.body.append(p);
        const c = getComputedStyle(p).backgroundColor;
        p.remove();
        return c;
      };
      return [...document.querySelectorAll('.analytics-split .analytics-panel')].map((panel) => ({
        title: panel.querySelector('h2').textContent.replace(/\s+/g, ' '),
        rows: [...panel.querySelectorAll('.analytics-session')].map((row) => {
          const track = row.querySelector('.row-track')?.getBoundingClientRect(),
            bar = row.querySelector('.row-bar')?.getBoundingClientRect(),
            box = row.getBoundingClientRect(),
            harness = ['claude', 'codex'].find((h) => row.classList.contains('h-' + h));
          return {
            tag: row.tagName,
            href: row.getAttribute('href'),
            ratio: track && bar ? bar.width / track.width : null,
            track: track?.width ?? 0,
            right: box.right,
            value: row.querySelector('.session-value').textContent,
            figureRight: Math.max(
              text(row.querySelector('.session-value')),
              text(row.querySelector('.hname')),
            ),
            harness,
            colour: row.querySelector('.row-bar')
              ? getComputedStyle(row.querySelector('.row-bar')).backgroundColor
              : null,
            expected: harness ? token(harness) : null,
          };
        }),
      }));
    }));
    r.expect(
      lists.length === 3,
      mode + ': expected three top-session lists: ' + lists.map((l) => l.title).join(' | '),
    );
    for (const list of lists) {
      const spec = TOP[list.title];
      if (!spec) {
        r.expect(false, mode + ': unexpected list ' + list.title);
        continue;
      }
      const max = Math.max(...spec.values);
      r.expect(
        list.rows.length === spec.values.length,
        mode +
          ": '" +
          list.title +
          "' has " +
          list.rows.length +
          ' rows, want ' +
          spec.values.length,
      );
      list.rows.forEach((row, i) => {
        const want = spec.values[i] / max;
        r.expect(
          row.ratio != null && Math.abs(row.ratio - want) <= 0.02,
          mode +
            ": '" +
            list.title +
            "' row " +
            (i + 1) +
            ' bar is ' +
            (row.ratio == null ? 'missing' : (row.ratio * 100).toFixed(1) + ' %') +
            ' of its track, want ' +
            (want * 100).toFixed(1) +
            ' % (' +
            row.value +
            ')',
        );
        r.expect(
          row.figureRight <= Math.min(row.right, res.fit.vw - 16) + 0.5,
          mode +
            ": '" +
            list.title +
            "' row " +
            (i + 1) +
            ' figure runs past the row or the gutter: ' +
            JSON.stringify(row),
        );
        r.expect(
          row.track >= 40,
          mode +
            ": '" +
            list.title +
            "' row " +
            (i + 1) +
            ' bar is ' +
            row.track +
            'px wide, want at least 40',
        );
        r.expect(
          row.tag === 'A' && /^\/s\/(claude|codex)\/./.test(row.href ?? ''),
          mode +
            ": '" +
            list.title +
            "' row " +
            (i + 1) +
            ' is not a link to its session: ' +
            JSON.stringify({ tag: row.tag, href: row.href }),
        );
        r.expect(
          row.harness && row.colour === row.expected,
          mode +
            ": '" +
            list.title +
            "' row " +
            (i + 1) +
            ' bar is not the harness colour: ' +
            JSON.stringify({ colour: row.colour, expected: row.expected, harness: row.harness }),
        );
      });
    }
    if (stacked) {
      await page.evaluate(() =>
        document.querySelector('.analytics-split').scrollIntoView({ block: 'start' }),
      );
      await page.screenshot({ path: path.join(shots, 'breakdown-390-light-top-sessions.png') });
    } else {
      await page.screenshot({ path: path.join(shots, 'breakdown-1280-light-top-sessions.png') });
    }
    // A keyboard reader's focus on a top-session link survives the next redraw (a new answer from the server), on the same row.
    if (stacked) {
      const before = await page.evaluate(() => {
        const rows = [...document.querySelectorAll('.analytics-split .analytics-session.ranked')],
          row = rows[7];
        row.focus();
        row.__old = true;
        return { index: rows.indexOf(row), focused: document.activeElement === row };
      });
      r.expect(before.focused, mode + ': a top-session link cannot take focus');
      page.fakeState.n++;
      const redrawn = await page
        .waitForResponse(
          (response) =>
            new URL(response.url()).pathname === '/api/analytics' &&
            response.status() === 200 &&
            response.headers().etag?.endsWith('-fake' + page.fakeState.n),
          { timeout: 20000 },
        )
        .then(
          () => true,
          () => false,
        );
      await page.evaluate(
        () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
      );
      await page.waitForTimeout(400);
      const after = await page.evaluate(() => {
        const rows = [...document.querySelectorAll('.analytics-split .analytics-session.ranked')],
          a = document.activeElement;
        return { index: rows.indexOf(a), tag: a?.tagName, kept: !!a?.__old };
      });
      res.focus = { before, redrawn, after };
      r.expect(
        redrawn && after.kept && after.index === before.index,
        mode +
          ': focus on a top-session link did not survive a redraw: ' +
          JSON.stringify(res.focus),
      );
    }
    // A tap on a row opens its session (the link is still handled in the page).
    const first = page.locator('.analytics-split .analytics-session.ranked').first();
    if (await first.count()) {
      await first.click();
      await page
        .waitForFunction(() => history.state?.v === 'session', null, { timeout: 3000 })
        .catch(() => {});
    }
    res.opened = await page.evaluate(() => ({ path: location.pathname, state: history.state?.v }));
    r.expect(
      /^\/s\//.test(res.opened.path) && res.opened.state === 'session',
      mode + ': a top-session row did not open its session: ' + JSON.stringify(res.opened),
    );
    errors.push(...page.errors);
    await page.context().close();
  }

  // The same screens in the dark scheme, and the light and dark shots the visual pass reads: the strip pinned over the repo rows, the top lists.
  for (const [scheme, size, dark] of [
    ['390-dark', 'phone', true],
    ['1280-dark', 'desktop', true],
  ]) {
    const page = await open(browser, D, { size, dark });
    await scrollToRow(page, 'repo', 12, r, scheme);
    await page.screenshot({ path: path.join(shots, 'breakdown-' + scheme + '-repo.png') });
    await scrollToRow(page, 'machine', 2, r, scheme);
    await page.screenshot({ path: path.join(shots, 'breakdown-' + scheme + '-machine.png') });
    await page.evaluate(() =>
      document.querySelector('.analytics-split').scrollIntoView({ block: 'start' }),
    );
    await page.waitForTimeout(100);
    await page.screenshot({ path: path.join(shots, 'breakdown-' + scheme + '-top-sessions.png') });
    errors.push(...page.errors);
    await page.context().close();
  }
  r.expect(errors.length === 0, 'page errors: ' + errors.join(' | '));
  r.results = results;
  return r.done();
}

// What an embedding page can do with the viewer: listen for `semon:ended` (and cancel it), ask for a poll with
// `semon:refresh`, watch `semon:polled`, and hand it an account menu in `window.semonEmbed.account`. Runs on the primary
// fixture's viewer, answering the polls itself (a real 304, a full 200, a 500, a 403, or one held open) through Playwright
// routes:
//   - cancelling `semon:ended` keeps the "Session ended" note away when the server answers 403; not cancelling shows it, and the
//     event says which status ended the session; a refresh after that polls nothing;
//   - dispatching `semon:refresh` during a backoff, a second after the last poll started, polls within 100 ms, and the poll
//     after it comes 2 s later, not at the backed-off delay;
//   - the refresh floor: a listener that refreshes on every `semon:polled` polls at most about once a second, polls at least a
//     second apart, and the same against a server answering 500, whose backoff still holds once the listener stops;
//   - refreshes while a poll is in flight add exactly one follow-up poll, and never a second request in flight;
//   - a refresh before the first model has loaded polls nothing;
//   - `semon:polled` fires exactly once per poll (the n-th event sees n polls started), says `ok` for a 200 and a 304, and not
//     for a 500 or a 403;
//   - `window.semonEmbed.account`, set before the page loads, gives the account menu, all text (no markup made from the name);
//     an href that is not a same-origin path (`javascript:`, `//host`, a backslash) rejects the menu, the same as a server's;
//     the menu is a copy, so changing the embedding page's object afterwards changes nothing; a getter that throws leaves no
//     menu and no page error;
//   - a menu the server provides wins over the embedding page's; an invalid one from the server leaves the embedding page's;
//   - on a phone the open menu floats above its row at the drawer's foot: inside the screen, above the row, the nav's boxes
//     unmoved, 44 px rows, Sign out in a section of its own, no row repeating a nav row's label; a tap outside it, Esc and the
//     back gesture each close it and leave the drawer open on the same page, and a tap outside it does not reach the drawer;
//     focus goes to its first row on open and back to its button on close; a live update while it is open leaves it open
//     (and is drawn once it closes); swiping the drawer shut closes it and leaves no history entry behind; and Back from a
//     page one of its items opened lands on this page with the menu closed, not on a dead entry for the menu;
//   - on desktop, a redraw of the top bar with the menu open ("N errors" on a session) closes the menu properly: the next
//     click on the avatar opens it (a menu only detached would leave it counted open, and every live update waiting).
// Screenshots of the open menu, at 1280 and 390 in light and dark, are written to out/embed/.
import fs from 'node:fs';
import path from 'node:path';
import { ENV, context, settled, reporter, closePage } from '../lib.mjs';

const OUT = path.join(ENV.out, 'embed');
fs.mkdirSync(OUT, { recursive: true });

const ENDED = 'Session ended: reload with the printed URL';
const account = (over = {}) => ({
  name: 'Embed <b>Ada</b> Lovelace',
  login: 'ada',
  initials: 'AL',
  workspaces: [
    { name: 'Engine room', role: 'owner', current: true, switch_href: '/embed/switch/1' },
  ],
  links: [
    { label: 'Profile', href: '/embed/profile', method: 'get', danger: false },
    { label: 'Sign out', href: '/embed/sign-out', method: 'post', danger: true },
  ],
  ...over,
});
const sleep = (n) => new Promise((resolve) => setTimeout(resolve, n));

// A page on the fixture's viewer. `state.mode` picks how a poll is answered: "pass" (the server's own answer, a 304 when
// nothing changed), "full" (a 200 with the whole model), "500" or "403". The boot request, the only one without `since`, goes
// through, with `serverAccount` added to the model when given (null: an invalid account, to test the fallback).
// `state.mode` "hold" keeps a poll open until `state.gate` resolves; `state.bootGate`, when set, holds the boot request the same
// way (and `open` returns before the page has drawn). `embed: "throw"` sets a `semonEmbed` whose `account` getter throws.
async function open(
  browser,
  { embed, state = { mode: 'pass' }, serverAccount, size = 'desktop', dark = false, at = '/' } = {},
) {
  const ctx = await context(browser, { size, dark });
  await ctx.addInitScript((embedded) => {
    if (embedded === 'throw')
      Object.defineProperty(window, 'semonEmbed', {
        value: {
          get account() {
            throw new Error('embed getter');
          },
        },
      });
    else if (embedded) window.semonEmbed = embedded;
    window.__ev = [];
    window.__fetches = [];
    window.__cancel = false;
    // Each event notes how many polls had started when it fired.
    for (const type of ['semon:ended', 'semon:polled']) {
      window.addEventListener(type, (e) => {
        if (type === 'semon:ended' && window.__cancel) e.preventDefault();
        window.__ev.push({
          type,
          detail: e.detail,
          cancelable: e.cancelable,
          prevented: e.defaultPrevented,
          fetches: window.__fetches.length,
        });
      });
    }
    const orig = window.fetch;
    window.fetch = function (...a) {
      if (
        (() => {
          const url = new URL(String(a[0]), location.href);
          return url.pathname === '/api/model' && url.searchParams.has('since');
        })()
      )
        window.__fetches.push(performance.now());
      return orig.apply(window, a);
    };
  }, embed ?? null);
  const page = await ctx.newPage();
  page.errors = [];
  page.on('pageerror', (e) => page.errors.push(e.message.split('\n')[0]));
  page.setDefaultTimeout(8000);
  await page.route(/.*/, (r) =>
    r
      .request()
      .url()
      .startsWith(ENV.base + '/')
      ? r.fallback()
      : r.abort(),
  );
  await page.route('**/api/model**', async (route) => {
    const url = new URL(route.request().url());
    if (!url.searchParams.has('since')) {
      if (state.bootGate) await state.bootGate;
      if (serverAccount === undefined) return route.continue();
      const model = await (await route.fetch()).json();
      return route.fulfill({
        status: 200,
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ ...model, account: serverAccount }),
      });
    }
    if (state.mode === 'hold') {
      await state.gate;
      return route.continue();
    }
    if (state.mode === '500') return route.fulfill({ status: 500, body: 'no' });
    if (state.mode === '403')
      return route.fulfill({ status: 403, contentType: 'text/plain', body: 'Forbidden' });
    if (state.mode === 'full') {
      url.searchParams.delete('since');
      return route.fulfill({
        status: 200,
        headers: { 'content-type': 'application/json' },
        body: await (await route.fetch({ url: url.toString() })).text(),
      });
    }
    return route.continue();
  });
  await page.goto(ENV.base + at + '?t=' + ENV.token, { waitUntil: 'load' });
  if (!state.bootGate) await settled(page);
  return page;
}
const events = (page, type) => page.evaluate((t) => window.__ev.filter((e) => e.type === t), type);
// Poll start times (page clock) since `from`.
const fetchesSince = (page, from) =>
  page.evaluate((t) => window.__fetches.filter((x) => x >= t), from);
const now = (page) => page.evaluate(() => performance.now());
const refresh = (page) => page.evaluate(() => dispatchEvent(new Event('semon:refresh')));
const minGap = (xs) => xs.slice(1).reduce((m, x, i) => Math.min(m, x - xs[i]), Infinity);
// Exactly once per poll: the n-th `semon:polled` fired when n polls had started.
const oncePerPoll = async (page) =>
  (await events(page, 'semon:polled')).every((e, i) => e.fetches === i + 1);
// The next `semon:polled` after the `n` seen so far.
const nextPolled = async (page, n) => {
  await page.waitForFunction(
    (k) => window.__ev.filter((e) => e.type === 'semon:polled').length > k,
    n,
    { timeout: 12000 },
  );
  return (await events(page, 'semon:polled'))[n];
};

export default async function embedCheck(browser) {
  const r = reporter('embed'),
    R = r.results;
  r.expect(!!ENV.base && !!ENV.token, 'the primary fixture server is required');

  // ---- semon:polled and semon:refresh ----
  {
    const state = { mode: 'pass' },
      page = await open(browser, { state });
    let n = 0;
    const p304 = await nextPolled(page, n++);
    r.expect(
      p304.detail?.ok === true,
      'semon:polled after a 304 should say ok: ' + JSON.stringify(p304),
    );
    state.mode = 'full';
    const p200 = await nextPolled(page, n++);
    r.expect(
      p200.detail?.ok === true,
      'semon:polled after a 200 should say ok: ' + JSON.stringify(p200),
    );
    state.mode = '500';
    const p500 = await nextPolled(page, n++);
    r.expect(
      p500.detail?.ok === false,
      'semon:polled after a 500 should not say ok: ' + JSON.stringify(p500),
    );
    R.polled = [p304.detail, p200.detail, p500.detail];

    // The failure backed the next poll off to 4 s. A second after that poll started, a refresh polls at once, and the 304 that
    // follows resets the delay to 2 s.
    state.mode = 'pass';
    await sleep(1200);
    const before = await page.evaluate(() => {
      const t = performance.now();
      window.__count = window.__fetches.length;
      dispatchEvent(new Event('semon:refresh'));
      return t;
    });
    await page
      .waitForFunction(
        (k) => window.__fetches.length > k,
        await page.evaluate(() => window.__count),
        { timeout: 3000 },
      )
      .catch(() => {});
    const fetches = await page.evaluate(() => window.__fetches.slice(window.__count));
    R.refreshDelay = fetches.length ? Math.round(fetches[0] - before) : null;
    r.expect(
      fetches.length > 0 && fetches[0] - before <= 100,
      'semon:refresh during a backoff should poll within 100 ms: ' + R.refreshDelay,
    );
    await page
      .waitForFunction(
        (k) => window.__fetches.length > k + 1,
        await page.evaluate(() => window.__count),
        { timeout: 3500 },
      )
      .catch(() => {});
    const after = await page.evaluate(() => window.__fetches.slice(window.__count));
    R.afterRefresh = after.length > 1 ? Math.round(after[1] - after[0]) : null;
    r.expect(
      after.length > 1 && after[1] - after[0] < 3000,
      'the poll after a refresh should come 2 s later, not at the backed-off delay: ' +
        R.afterRefresh,
    );
    r.expect(
      await oncePerPoll(page),
      'semon:polled should fire exactly once per poll: ' +
        JSON.stringify(await events(page, 'semon:polled')),
    );
    r.expect(page.errors.length === 0, 'page errors: ' + page.errors.join('; '));
    await closePage(page);
  }

  // ---- The refresh floor: a listener that refreshes on every semon:polled, against a healthy server and a failing one ----
  for (const mode of ['pass', '500']) {
    const state = { mode: 'pass' },
      page = await open(browser, { state });
    await nextPolled(page, 0);
    state.mode = mode;
    const t0 = await now(page);
    await page.evaluate(() => {
      window.__loop = () => dispatchEvent(new Event('semon:refresh'));
      addEventListener('semon:polled', window.__loop);
      dispatchEvent(new Event('semon:refresh'));
    });
    await sleep(5000);
    await page.evaluate(() => removeEventListener('semon:polled', window.__loop));
    const polls = (await fetchesSince(page, t0)).filter((x) => x < t0 + 5000),
      gap = minGap(polls);
    const tag = mode === 'pass' ? 'healthy' : '500';
    R['floor ' + tag] = { polls: polls.length, minGap: Math.round(gap) };
    r.expect(
      polls.length >= 3 && polls.length <= 6,
      'refresh on every semon:polled (' +
        tag +
        ') should poll about once a second over 5 s: ' +
        polls.length,
    );
    r.expect(
      gap >= 950,
      'polls driven by refresh (' +
        tag +
        ') should start at least a second apart: ' +
        Math.round(gap),
    );
    if (mode === '500') {
      // The pending floor poll runs (and fails); after it, the backoff the failures set holds: nothing for the next 3 s.
      await sleep(1500);
      const t1 = await now(page);
      await sleep(3000);
      const later = await fetchesSince(page, t1);
      R['floor 500 after'] = later.length;
      r.expect(
        later.length === 0,
        'once refreshes stop, the backoff from the 500s should hold, not a 2 s poll: ' +
          later.length,
      );
    }
    r.expect(await oncePerPoll(page), 'semon:polled once per poll (' + tag + ')');
    r.expect(
      page.errors.length === 0,
      'page errors (floor ' + tag + '): ' + page.errors.join('; '),
    );
    await closePage(page);
  }

  // ---- Refreshes while a poll is in flight: one follow-up ----
  {
    const state = { mode: 'pass' },
      page = await open(browser, { state });
    await nextPolled(page, 0);
    let release;
    state.gate = new Promise((resolve) => {
      release = resolve;
    });
    state.mode = 'hold';
    const k = await page.evaluate(() => window.__fetches.length);
    await refresh(page);
    await page.waitForFunction((n) => window.__fetches.length > n, k, { timeout: 3000 });
    for (let i = 0; i < 3; i++) await refresh(page);
    await sleep(300);
    const during = (await page.evaluate(() => window.__fetches.length)) - k;
    state.mode = 'pass';
    release();
    await sleep(1500);
    const total = (await page.evaluate(() => window.__fetches.length)) - k;
    R.inFlight = { during, total };
    r.expect(during === 1, 'refreshes during a poll should not start a second request: ' + during);
    r.expect(
      total === 2,
      'refreshes during a poll should add exactly one follow-up poll: ' + (total - 1),
    );
    r.expect(await oncePerPoll(page), 'semon:polled once per poll (in flight)');
    r.expect(page.errors.length === 0, 'page errors (in flight): ' + page.errors.join('; '));
    await closePage(page);
  }

  // ---- A refresh before the first model has loaded ----
  {
    let release;
    const state = {
      mode: 'pass',
      bootGate: new Promise((resolve) => {
        release = resolve;
      }),
    };
    const page = await open(browser, { state });
    for (let i = 0; i < 3; i++) await refresh(page);
    await sleep(300);
    const held = await page.evaluate(() => window.__fetches.length);
    release();
    state.bootGate = null;
    await settled(page);
    await sleep(1000);
    const polls = await page.evaluate(() => window.__fetches.length);
    R.beforeLoad = { held, polls };
    r.expect(
      held === 0 && polls === 0,
      'a refresh before the first model load should poll nothing, then or once it loads (the first poll is 2 s after it): ' +
        JSON.stringify(R.beforeLoad),
    );
    r.expect(page.errors.length === 0, 'page errors (before load): ' + page.errors.join('; '));
    await closePage(page);
  }

  // ---- semon:ended ----
  for (const cancel of [true, false]) {
    const page = await open(browser, { state: { mode: '403' } });
    await page.evaluate((c) => {
      window.__cancel = c;
    }, cancel);
    await page.waitForFunction(() => window.__ev.some((e) => e.type === 'semon:ended'), null, {
      timeout: 8000,
    });
    await sleep(300);
    const [ended] = await events(page, 'semon:ended'),
      note = await page.evaluate(() => document.querySelector('.livenote')?.textContent ?? null);
    const tag = cancel ? 'cancelled' : 'not cancelled';
    R['ended ' + tag] = { detail: ended.detail, cancelable: ended.cancelable, note };
    r.expect(
      ended.cancelable === true && ended.detail?.status === 403,
      'semon:ended (' + tag + ') should be cancelable and carry the 403: ' + JSON.stringify(ended),
    );
    r.expect(
      cancel ? note === null : note === ENDED,
      'semon:ended ' + tag + ': the note read ' + JSON.stringify(note),
    );
    const polls = await page.evaluate(() => window.__fetches.length);
    await refresh(page);
    await sleep(2500);
    r.expect(
      (await page.evaluate(() => window.__fetches.length)) === polls,
      'polling should stop after a 403, and a refresh should not start it (' + tag + ')',
    );
    const polled = await events(page, 'semon:polled');
    r.expect(
      polled.length === polls && polled.at(-1)?.detail?.ok === false && (await oncePerPoll(page)),
      'semon:polled once per poll, the last with ok:false after the 403 (' +
        tag +
        '): ' +
        JSON.stringify(polled),
    );
    r.expect(page.errors.length === 0, 'page errors (' + tag + '): ' + page.errors.join('; '));
    await closePage(page);
  }

  // ---- The account menu from the embedding page ----
  // Hostile values traverse the served production bundle's copied model and Preact props.
  for (const size of ['phone', 'desktop']) {
    const hostile = '<img src=x onerror="window.__accountXss=1">';
    const page = await open(browser, {
      size,
      embed: {
        account: account({
          name: hostile,
          login: '</span><script>window.__accountXss=2</script>',
          initials: '<b>',
          workspaces: [
            {
              name: hostile,
              role: '<svg onload="window.__accountXss=3">',
              current: true,
              switch_href: '/embed/switch/1',
            },
          ],
          links: [{ label: hostile, href: '/embed/profile', method: 'get', danger: false }],
        }),
      },
    });
    if (size === 'phone') await page.locator('#lead-btn').click();
    await page
      .locator(size === 'phone' ? '#account-drawer .account-trigger' : '#topbar .account-trigger')
      .click();
    const state = await page.evaluate(() => {
      const menu = document.querySelector('.account-popover');
      return {
        name: menu.querySelector('.account-name').textContent,
        markup: menu.querySelectorAll('script, img, svg, b, [style], [onerror], [onload]').length,
        executed: window.__accountXss ?? 0,
      };
    });
    r.expect(
      state.name === hostile && state.markup === 0 && state.executed === 0,
      'hostile account values stay text in served ' + size + ' menu: ' + JSON.stringify(state),
    );
    await closePage(page);
  }

  const menuOf = (page, scope = '#topbar') =>
    page.evaluate((sc) => {
      const menu = document.querySelector(sc + ' .account-popover');
      return menu
        ? {
            name: menu.querySelector('.account-name')?.textContent,
            login: menu.querySelector('.account-login-value')?.textContent,
            bold: menu.querySelectorAll('b').length,
            hrefs: [...menu.querySelectorAll('a[href]')].map((a) => a.getAttribute('href')),
            actions: [...menu.querySelectorAll('form')].map((f) => f.getAttribute('action')),
            workspaces: [...menu.querySelectorAll('.account-workspace-name')].map(
              (n) => n.textContent,
            ),
          }
        : null;
    }, scope);
  const openMenu = async (page) => {
    await page.locator('#topbar .account-avatar-button').click();
    return menuOf(page);
  };
  // On a phone the menu opens from the account row at the drawer's foot.
  const openPhoneMenu = async (page) => {
    await page.locator('#lead-btn').click();
    const footer = page.locator('#account-drawer');
    await footer.scrollIntoViewIfNeeded();
    await footer.locator('.account-trigger').click();
    // The menu opens above the row; bring the row's foot into view so the shot shows both.
    await footer.evaluate((w) => w.scrollIntoView({ block: 'end' }));
    return menuOf(page, '#account-drawer');
  };
  for (const dark of [false, true]) {
    const tag = '1280' + (dark ? '-dark' : '');
    const page = await open(browser, {
      embed: { account: account() },
      size: 'desktop',
      dark,
      at: '/s/claude/harbor',
    });
    const start = await page.evaluate(() => localStorage.getItem('semon.wide') === '1');
    await openMenu(page);
    const control = page.locator(
      '#topbar .account-popover [role="menuitemcheckbox"][data-pref="wide"]',
    );
    const before = await control.getAttribute('aria-checked');
    await control.evaluate((row) => {
      window.__accountDisplayNode = row;
      window.__accountWorkspaceNode = row
        .closest('.account-popover')
        .querySelector('.account-workspace-form');
    });
    await control.click();
    const changed = await page.evaluate(() => ({
      checked: document
        .querySelector('#topbar .account-popover [role="menuitemcheckbox"][data-pref="wide"]')
        ?.getAttribute('aria-checked'),
      wide: document.querySelector('#page').classList.contains('wide-mode'),
      saved: localStorage.getItem('semon.wide'),
      open: !!document.querySelector('#topbar .account-popover'),
      kept:
        window.__accountDisplayNode ===
        document.querySelector('#topbar .account-popover [data-pref="wide"]'),
      focused: document.activeElement === window.__accountDisplayNode,
      workspaceKept:
        window.__accountWorkspaceNode ===
        document.querySelector('#topbar .account-popover .account-workspace-form'),
    }));
    const expected = !start;
    R['desktopWideMode ' + tag] = { before, changed };
    r.expect(
      before === String(start) &&
        changed.checked === String(expected) &&
        changed.wide === expected &&
        changed.saved === (expected ? '1' : '0') &&
        changed.open &&
        changed.kept &&
        changed.focused &&
        changed.workspaceKept,
      'the desktop Display switch should toggle wide mode, save it, and leave the menu open: ' +
        JSON.stringify(R['desktopWideMode ' + tag]),
    );
    await page.waitForTimeout(300); // the knob's slide is 160 ms; the shot shows the settled state
    await page.screenshot({ path: path.join(OUT, 'embed-menu-wide-on-' + tag + '.png') });
    await page.reload({ waitUntil: 'load' });
    await settled(page);
    await openMenu(page);
    const saved = await page.evaluate(() => ({
      checked: document
        .querySelector('#topbar .account-popover [role="menuitemcheckbox"][data-pref="wide"]')
        ?.getAttribute('aria-checked'),
      wide: document.querySelector('#page').classList.contains('wide-mode'),
    }));
    R['desktopWideModeReload ' + tag] = saved;
    r.expect(
      saved.checked === String(expected) && saved.wide === expected,
      'the desktop Display switch should show the saved state after reload: ' +
        JSON.stringify(saved),
    );
    r.expect(
      page.errors.length === 0,
      'page errors (desktop wide mode): ' + page.errors.join('; '),
    );
    await closePage(page);
  }
  for (const [size, dark] of [
    ['desktop', false],
    ['desktop', true],
    ['phone', false],
    ['phone', true],
  ]) {
    const tag = (size === 'desktop' ? '1280' : '390') + (dark ? '-dark' : '');
    const page = await open(browser, { embed: { account: account() }, size, dark });
    const menu = size === 'desktop' ? await openMenu(page) : await openPhoneMenu(page);
    const display = await page.evaluate(
      (scope) => {
        const menu = document.querySelector(scope + ' .account-popover');
        const sections = [...(menu?.querySelectorAll('.account-section') ?? [])].filter(
          (section) => section.querySelector('.account-heading')?.textContent.trim() === 'Display',
        );
        const rows = menu?.querySelectorAll('[role="menuitemcheckbox"][data-pref="wide"]') ?? [];
        return {
          topWideToggles: document.querySelectorAll('#topbar .wide-toggle').length,
          sections: sections.length,
          rows: rows.length,
          rowUnderHeading: rows.length === 1 && rows[0].closest('.account-section') === sections[0],
        };
      },
      size === 'desktop' ? '#topbar' : '#account-drawer',
    );
    if (size === 'desktop') {
      R['desktopDisplay ' + tag] = display;
      r.expect(
        display.topWideToggles === 0,
        'the desktop top bar should omit the wide mode button when an account menu is available (' +
          tag +
          ')',
      );
      r.expect(
        display.sections === 1 && display.rows === 1 && display.rowUnderHeading,
        'the desktop account menu should have one wide mode switch under Display (' +
          tag +
          '): ' +
          JSON.stringify(display),
      );
    } else {
      R['phoneDisplay ' + tag] = display;
      r.expect(
        display.sections === 0 && display.rows === 0,
        'the phone account menu should omit the Display section (' +
          tag +
          '): ' +
          JSON.stringify(display),
      );
    }
    R['embedMenu ' + tag] = menu;
    r.expect(
      menu?.name === 'Embed <b>Ada</b> Lovelace' && menu.login === 'ada' && menu.bold === 0,
      "the embedding page's account should show as text (" + tag + '): ' + JSON.stringify(menu),
    );
    r.expect(
      menu &&
        menu.hrefs.join() === '/embed/profile' &&
        menu.actions.join() === '/embed/switch/1,/embed/sign-out' &&
        menu.workspaces.join() === 'Engine room',
      "the embedding page's menu links (" + tag + '): ' + JSON.stringify(menu),
    );
    await page.screenshot({ path: path.join(OUT, 'embed-menu-' + tag + '.png') });
    r.expect(
      page.errors.length === 0,
      'page errors (embed menu ' + tag + '): ' + page.errors.join('; '),
    );
    await closePage(page);
  }
  // On a phone the menu floats above its row and never moves the drawer; outside, Esc and back close it.
  for (const dark of [false, true]) {
    const tag = '390' + (dark ? '-dark' : '');
    const live = { mode: 'pass' };
    const page = await open(browser, {
      embed: { account: account() },
      size: 'phone',
      dark,
      state: live,
    });
    await page.locator('#lead-btn').click();
    await page.waitForTimeout(350);
    const navBoxes = () =>
      page.evaluate(() =>
        [...document.querySelectorAll('#nav .nav-item')]
          .map((n) => {
            const b = n.getBoundingClientRect();
            return [b.left, b.top, b.width, b.height].map((x) => Math.round(x * 10) / 10);
          })
          .join(' '),
      );
    const state = () =>
      page.evaluate(() => ({
        open: !!document.querySelector('#account-drawer .account-popover'),
        drawer: document.body.classList.contains('drawer-open'),
        path: location.pathname,
        sheet: !!history.state?.sheet,
      }));
    const openIt = async () => {
      await page.locator('#account-drawer .account-trigger').click();
      await page.waitForSelector('#account-drawer .account-popover');
    };
    const before = await navBoxes(),
      path0 = (await state()).path;
    await openIt();
    const focusIn = await page.evaluate(
      () =>
        document
          .querySelector('#account-drawer .account-popover')
          .contains(document.activeElement) &&
        document.activeElement.classList.contains('account-menu-row'),
    );
    r.expect(focusIn, 'opening the phone menu focuses its first row (' + tag + ')');
    const facts = await page.evaluate(() => {
      const menu = document.querySelector('#account-drawer .account-popover'),
        trigger = document.querySelector('#account-drawer .account-trigger');
      const m = menu.getBoundingClientRect(),
        t = trigger.getBoundingClientRect();
      const nav = [...document.querySelectorAll('#nav .nav-item > span:first-of-type')].map((x) =>
        x.textContent.trim(),
      );
      const rows = [...menu.querySelectorAll('.account-menu-row')].filter(
        (row) => row.getClientRects().length,
      );
      const labels = rows.map((row) =>
        (row.querySelector('.account-workspace-name') ?? row).textContent.trim(),
      );
      const danger = menu.querySelector('.account-menu-row.danger');
      return {
        menu: [m.left, m.top, m.right, m.bottom].map((x) => Math.round(x * 10) / 10),
        triggerTop: Math.round(t.top * 10) / 10,
        vw: innerWidth,
        vh: innerHeight,
        rows: rows.map((row) => Math.round(row.getBoundingClientRect().height)),
        labels,
        nav,
        repeated: labels.filter((label) => nav.includes(label)),
        dangerApart:
          !!danger &&
          danger.closest('.account-section')?.querySelectorAll('.account-menu-row').length === 1,
      };
    });
    R['phoneMenu ' + tag] = facts;
    r.expect(
      facts.menu[0] >= 0 &&
        facts.menu[1] >= 0 &&
        facts.menu[2] <= facts.vw &&
        facts.menu[3] <= facts.vh,
      'the phone menu fits the screen (' + tag + '): ' + JSON.stringify(facts),
    );
    r.expect(
      facts.menu[3] <= facts.triggerTop,
      'the phone menu sits above its row (' + tag + '): ' + JSON.stringify(facts),
    );
    r.expect(
      (await navBoxes()) === before,
      'opening the phone menu moves nothing in the drawer (' + tag + ')',
    );
    r.expect(
      facts.rows.length >= 3 && facts.rows.every((h) => h >= 44),
      "the phone menu's rows are 44 px to tap (" + tag + '): ' + JSON.stringify(facts.rows),
    );
    r.expect(
      facts.repeated.length === 0,
      'no phone menu row repeats a nav row (' + tag + '): ' + JSON.stringify(facts),
    );
    r.expect(facts.dangerApart, 'Sign out sits in a section of its own (' + tag + ')');
    await page.screenshot({ path: path.join(OUT, 'embed-phone-menu-' + tag + '.png') });
    // A tap outside the menu closes it and reaches nothing under it (here the drawer's first nav row).
    const home = await page.locator('#nav .nav-item').first().boundingBox();
    await page.mouse.click(home.x + home.width / 2, home.y + home.height / 2);
    await page.waitForTimeout(250);
    const tapped = await state();
    r.expect(
      !tapped.open && tapped.drawer && tapped.path === path0 && !tapped.sheet,
      'a tap outside closes the phone menu and nothing else (' +
        tag +
        '): ' +
        JSON.stringify(tapped),
    );
    await openIt();
    await page.keyboard.press('Escape');
    await page.waitForTimeout(250);
    const escaped = await state();
    r.expect(
      !escaped.open && escaped.drawer && escaped.path === path0 && !escaped.sheet,
      'Esc closes the phone menu and leaves the drawer open (' +
        tag +
        '): ' +
        JSON.stringify(escaped),
    );
    r.expect(
      await page.evaluate(
        () => document.activeElement === document.querySelector('#account-drawer .account-trigger'),
      ),
      'closing the phone menu returns focus to its button (' + tag + ')',
    );
    // A live update while the menu is open leaves it open (the same element, under the finger), and is drawn once it closes.
    await openIt();
    await page.evaluate(() => {
      document.querySelector('#account-drawer .account-popover').dataset.probe = 'kept';
    });
    const seen = (await events(page, 'semon:polled')).length;
    live.mode = 'full';
    await refresh(page);
    const updated = await nextPolled(page, seen);
    live.mode = 'pass';
    await page.waitForTimeout(200);
    const during = await page.evaluate(
      () => document.querySelector('#account-drawer .account-popover')?.dataset.probe ?? null,
    );
    R['phoneMenuLive ' + tag] = { updated: updated?.detail, during };
    r.expect(
      updated?.detail?.ok === true && during === 'kept',
      'a live update leaves the open phone menu as it was (' +
        tag +
        '): ' +
        JSON.stringify(R['phoneMenuLive ' + tag]),
    );
    await page.keyboard.press('Escape');
    await page.waitForTimeout(250);
    const afterLive = await state();
    r.expect(
      !afterLive.open && afterLive.drawer && !afterLive.sheet,
      'the phone menu closes after a live update, with no entry left (' +
        tag +
        '): ' +
        JSON.stringify(afterLive),
    );
    // Swiping the drawer shut closes the menu and takes its history entry.
    await openIt();
    await page.evaluate(() => {
      const on = document.querySelector('#account-drawer .account-backdrop'),
        at = (x) => new Touch({ identifier: 1, target: on, clientX: x, clientY: 300 });
      on.dispatchEvent(new TouchEvent('touchstart', { touches: [at(250)], bubbles: true }));
      on.dispatchEvent(new TouchEvent('touchmove', { touches: [at(150)], bubbles: true }));
    });
    await page.waitForTimeout(350);
    const swiped = await state();
    r.expect(
      !swiped.open && !swiped.drawer && swiped.path === path0 && !swiped.sheet,
      'swiping the drawer shut closes the phone menu with no entry left (' +
        tag +
        '): ' +
        JSON.stringify(swiped),
    );
    await page.locator('#lead-btn').click();
    await page.waitForTimeout(350);
    await openIt();
    await page.evaluate(() => history.back());
    await page.waitForTimeout(300);
    const backed = await state();
    r.expect(
      !backed.open && backed.drawer && backed.path === path0,
      'back closes the phone menu and stays on the page (' + tag + '): ' + JSON.stringify(backed),
    );
    // Leaving from an item: Back from the page it opened lands here with the menu closed, not on an entry for the menu.
    await openIt();
    await Promise.all([
      page.waitForURL((url) => url.pathname === '/embed/profile'),
      page.locator('#account-drawer .account-popover a[href="/embed/profile"]').click(),
    ]);
    await page.goBack({ waitUntil: 'load' });
    await page.waitForTimeout(300);
    const returned = await state();
    r.expect(
      returned.path === path0 && !returned.sheet && !returned.open,
      'Back from a page a menu item opened lands on this page, menu closed (' +
        tag +
        '): ' +
        JSON.stringify(returned),
    );
    r.expect(
      page.errors.length === 0,
      'page errors (phone menu ' + tag + '): ' + page.errors.join('; '),
    );
    await closePage(page);
  }
  // A top bar redrawn with the desktop menu open ("N errors" stops its click there and redraws the bar) closes the menu
  // properly: the next click on the avatar opens it at once.
  {
    const page = await open(browser, { embed: { account: account() }, at: '/s/claude/harbor' });
    await page.waitForSelector('#topbar .lab-errs');
    await page.locator('#topbar .account-avatar-button').click();
    await page.waitForSelector('#topbar .account-popover');
    await page.locator('#topbar .lab-errs').click();
    await page.waitForSelector('#topbar .errnav-count');
    const gone = await page.evaluate(() => !document.querySelector('.account-popover'));
    await page.locator('#topbar .account-avatar-button').click();
    await page.waitForTimeout(200);
    const reopened = await page.evaluate(
      () => !!document.querySelector('#topbar .account-popover'),
    );
    R.menuAfterBarRedraw = { gone, reopened };
    r.expect(
      gone && reopened,
      'a top bar redrawn with the menu open closes it, and the avatar opens it again with one click: ' +
        JSON.stringify(R.menuAfterBarRedraw),
    );
    r.expect(
      page.errors.length === 0,
      'page errors (menu and N errors): ' + page.errors.join('; '),
    );
    await closePage(page);
  }
  // The menu is a copy: changing the embedding page's object after it was read changes nothing on screen.
  {
    const page = await open(browser, { embed: { account: account() } });
    await page.evaluate(() => {
      const a = window.semonEmbed.account;
      a.name = 'x'.repeat(200);
      a.links[0].href = '//evil.example/x';
      a.workspaces[0].name = 'Changed';
    });
    const menu = await openMenu(page);
    R.embedCopy = menu;
    r.expect(
      menu?.name === 'Embed <b>Ada</b> Lovelace' &&
        menu.hrefs.join() === '/embed/profile' &&
        menu.workspaces.join() === 'Engine room',
      "the account menu should be a copy of the embedding page's object: " + JSON.stringify(menu),
    );
    await closePage(page);
  }
  // A getter that throws: no menu, and the page still draws and polls.
  {
    const page = await open(browser, { embed: 'throw' });
    await nextPolled(page, 0);
    const widgets = await page.locator('.account-widget').count();
    R.embedThrows = { widgets, errors: page.errors };
    r.expect(
      widgets === 0 && page.errors.length === 0,
      'a throwing semonEmbed.account getter should leave no menu and no page error: ' +
        JSON.stringify(R.embedThrows),
    );
    await closePage(page);
  }
  for (const [what, over] of [
    [
      'a javascript: link',
      { links: [{ label: 'Bad', href: 'javascript:alert(1)', method: 'get', danger: false }] },
    ],
    [
      'a protocol-relative link',
      { links: [{ label: 'Bad', href: '//evil.example/x', method: 'get', danger: false }] },
    ],
    [
      'a backslash link',
      { links: [{ label: 'Bad', href: '/ok\\evil', method: 'get', danger: false }] },
    ],
    [
      'a workspace switch to //host',
      {
        workspaces: [
          { name: 'Bad', role: '', current: true, switch_href: '//evil.example/switch' },
        ],
      },
    ],
    ['a javascript: avatar', { avatar_href: 'javascript:alert(1)' }],
  ]) {
    const page = await open(browser, { embed: { account: account(over) } });
    const widgets = await page.locator('.account-widget').count(),
      bad = await page
        .locator(
          'a[href^="javascript:"], a[href^="//"], form[action^="//"], img[src^="javascript:"]',
        )
        .count();
    R['invalid ' + what] = { widgets, bad };
    r.expect(
      widgets === 0 && bad === 0,
      "the embedding page's account with " +
        what +
        " should be dropped like a server's, and nothing built from it: " +
        JSON.stringify({ widgets, bad }),
    );
    await closePage(page);
  }
  {
    const server = account({
      name: 'Server Grace',
      login: 'grace',
      initials: 'SG',
      workspaces: [{ name: 'Bridge', role: 'admin', current: true, switch_href: '/server/switch' }],
      links: [{ label: 'Server profile', href: '/server/profile', method: 'get', danger: false }],
    });
    const page = await open(browser, { embed: { account: account() }, serverAccount: server });
    const menu = await openMenu(page);
    R.serverWins = menu;
    r.expect(
      menu?.name === 'Server Grace' && menu.hrefs.join() === '/server/profile',
      "a server-provided account should win over the embedding page's: " + JSON.stringify(menu),
    );
    r.expect(page.errors.length === 0, 'page errors (server wins): ' + page.errors.join('; '));
    await closePage(page);
    // A server account the rules reject leaves the embedding page's.
    const invalid = {
      ...server,
      links: [{ label: 'Bad', href: '//evil.example/x', method: 'get', danger: false }],
    };
    const fallback = await open(browser, { embed: { account: account() }, serverAccount: invalid });
    const menu2 = await openMenu(fallback);
    R.serverInvalid = menu2;
    r.expect(
      menu2?.name === 'Embed <b>Ada</b> Lovelace',
      "an invalid server account should leave the embedding page's: " + JSON.stringify(menu2),
    );
    await closePage(fallback);
  }

  return r.done();
}

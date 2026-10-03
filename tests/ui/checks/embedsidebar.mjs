// The viewer's sidebar on an embedding page (docs/shell.md, "The viewer's sidebar on an embedding page"). Two pages of their own load
// /shell.js and then /viewer.js with their .app marked data-viewer="sidebar", each served from the primary fixture's origin with the
// viewer's own headers (CSP included), as shell.mjs serves the gallery:
//   - "full", tests/ui/shell-sidebar.html: drawn with shell::session_sidebar (a Rust test holds it to the API), with the signed-in
//     skeleton's top bar, #main and #page;
//   - "bare", the page docs/shell.md gives as the whole of such a page, with its comment replaced by the same sidebar markup: no
//     #main, #page or #topbar.
// On both, at 390 and 1280:
//   - the viewer's script draws the navigation (its native links, with Home's badge and no Sessions count, as the viewer's page has) and the
//     Recent list from /api/model: the same rows, in the same order, with the same text and dots (state, and a parent's attention
//     dot), as the viewer's own page, and only Machines is current;
//   - nothing outside the sidebar's own parts (header, nav, Recent list) changes: the document, serialized without those
//     parts, equals the page as served (so the body gets no new children and no element outside them a class, attribute or child),
//     the address is the page's, and no page error is thrown. At 1280 a wide page and a collapsed rail are saved, as a desktop reader
//     may have them: the page gets neither, and there is no rail toggle;
//   - a live poll that changes the model (a lower row's session active just now) redraws the list without moving a row (#126 holds
//     the reorder) and leaves the page as served; at 390 on "full", opening the drawer then re-sorts it, that row first.
// On "full", light and dark: the nav, Recent label and first row sit where the viewer's do (on a phone, in the open drawer), with the
// same type, and screenshots of both go to out/embedsidebar/. At 390 light: "/" opens no drawer and takes no focus; the phone's
// "All N" sheet opens and closes (Esc, which leaves the drawer open) without a history entry; a tap on a row opens its session's page
// in the viewer by a full page load (a marker set on the page is gone). The viewer's "/" shortcut opens Sessions and focuses its search;
// that page's search filters its rows and a direct `?q=` link pre-fills it. On "bare" at 1280: a click on a row does the same; a
// refused poll (403) leaves a note under the list and nothing else.
import fs from 'node:fs';
import path from 'node:path';
import { ENV, context, served, reporter } from '../lib.mjs';

const OUT = path.join(ENV.out, 'embedsidebar');
fs.mkdirSync(OUT, { recursive: true });
const FULL = fs.readFileSync(new URL('../shell-sidebar.html', import.meta.url), 'utf8');
// The sidebar markup the full page draws with session_sidebar, put where the documented page has its comment.
const SIDEBAR = FULL.split(
  '<aside class="sidebar" id="sidebar" aria-label="Navigation">\n',
)[1].split('\n<div class="account">')[0];
const DOC = fs.readFileSync(new URL('../../../docs/shell.md', import.meta.url), 'utf8');
const BARE = DOC.split("## The viewer's sidebar on an embedding page\n")[1]
  .split('```html\n')[1]
  .split('```')[0]
  .replace('<!-- session_sidebar("Semon", nav) -->', SIDEBAR);
const PAGES = { full: ['/__shell-sidebar.html', FULL], bare: ['/__shell-sidebar-bare.html', BARE] };

// `saved`: the viewer's own layout settings a desktop reader may have saved (the wide page and the collapsed rail), which must not
// reach the embedding page.
async function embedPage(browser, which, { size, dark, saved = false }) {
  const [at, html] = PAGES[which];
  const ctx = await context(browser, { size, dark });
  if (saved)
    await ctx.addInitScript(() => {
      try {
        localStorage.setItem('semon.wide', '1');
        localStorage.setItem('semon.rail', '1');
      } catch {}
    });
  const page = await ctx.newPage();
  page.errors = [];
  page.on('pageerror', (e) => page.errors.push(e.message.split('\n')[0]));
  page.setDefaultTimeout(8000);
  await page.clock.setFixedTime(ENV.now);
  await page.route(/.*/, (r) =>
    r
      .request()
      .url()
      .startsWith(ENV.base + '/')
      ? r.continue()
      : r.abort(),
  );
  // Loading a stylesheet with the token sets the server's cookie and gives the headers the viewer's origin sends with a page.
  const sheet = await page.goto(ENV.base + '/shell.css?t=' + ENV.token, { waitUntil: 'load' });
  const headers = Object.fromEntries(
    Object.entries(sheet.headers()).filter(
      ([name]) =>
        ![
          'content-type',
          'content-length',
          'content-encoding',
          'etag',
          'set-cookie',
          'date',
          'connection',
          'transfer-encoding',
          'keep-alive',
        ].includes(name),
    ),
  );
  await page.route(ENV.base + at, (route) =>
    route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', headers, body: html }),
  );
  await page.goto(ENV.base + at, { waitUntil: 'load' });
  await page.waitForSelector('#lanes .srow', { state: 'attached' });
  await page.evaluate(() => document.fonts.ready);
  page.at = at;
  page.html = html;
  return page;
}

// The document serialized with the sidebar's own parts (what the viewer's script draws into) cut out, now and as served. Any other
// change, a new child of the body, a class or attribute on the page's elements, shows as a difference.
const untouched = (page) =>
  page.evaluate(
    ({ html, at }) => {
      const OWN = ['.sidebar-head', '#nav', '.side-h', '#side-list'];
      const text = (root) => {
        const c = root.cloneNode(true);
        for (const sel of OWN)
          c.querySelectorAll(sel).forEach((n) => n.replaceWith(document.createComment(sel)));
        return c.outerHTML;
      };
      const was = text(new DOMParser().parseFromString(html, 'text/html').documentElement),
        now = text(document.documentElement);
      let i = 0;
      while (i < was.length && was[i] === now[i]) i++;
      return {
        same: was === now && location.pathname === at,
        path: location.pathname,
        at: i,
        was: was.slice(Math.max(0, i - 80), i + 120),
        now: now.slice(Math.max(0, i - 80), i + 120),
      };
    },
    { html: page.html, at: page.at },
  );

const openDrawer = async (page) => {
  await page.click('#lead-btn');
  await page.waitForTimeout(350);
};
// Each row: its id, label and text, and its dots (the state dot, and a parent's attention dot, amber or red), by class.
const lanes = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll('#lanes .srow')].map((r) => ({
      id: r.dataset.id,
      label: r.getAttribute('aria-label'),
      text: r.textContent,
      dots: [...r.querySelectorAll('.dot, .kid-flag')].map((d) => d.className),
    })),
  );
const nav = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll('#nav .nav-item')].map((b) => ({
      tag: b.tagName,
      href: b.getAttribute('href'),
      label: b.querySelector(':scope > span:not(.cnt)')?.textContent ?? null,
      cnt: b.querySelector('.cnt')?.textContent ?? null,
      hot: !!b.querySelector('.cnt.hot'),
      current: b.getAttribute('aria-current'),
    })),
  );
// Where the sidebar's parts sit in it, and their type. The list's own height is left out: the embedding page's account row under it
// takes room the viewer's page (no account on this fixture) does not.
const layout = (page) =>
  page.evaluate(() => {
    const side = document.querySelector('#sidebar').getBoundingClientRect();
    const box = (sel) => {
      const e = document.querySelector(sel);
      if (!e) return null;
      const b = e.getBoundingClientRect();
      return { left: b.left - side.left, top: b.top - side.top, width: b.width, height: b.height };
    };
    const type = (sel) => {
      const e = document.querySelector(sel);
      if (!e) return null;
      const s = getComputedStyle(e);
      return { size: s.fontSize, weight: s.fontWeight, family: s.fontFamily, color: s.color };
    };
    const list = box('#side-list');
    return {
      sidebar: { width: side.width },
      nav: box('#nav'),
      navRow: box('#nav .nav-item'),
      recent: box('.side-h'),
      list: list && { left: list.left, top: list.top, width: list.width },
      row: box('#lanes .srow'),
      rowName: type('#lanes .srow .nm'),
      rowMeta: type('#lanes .srow-meta'),
      recentType: type('.side-h'),
      badge: type('#nav .cnt'),
    };
  });
function differences(a, b, at = '') {
  if (typeof a === 'number' && typeof b === 'number')
    return Math.abs(a - b) <= 0.5 ? [] : [at + ': ' + a + ' vs ' + b];
  if (a && b && typeof a === 'object' && typeof b === 'object')
    return [...new Set([...Object.keys(a), ...Object.keys(b)])].flatMap((k) =>
      differences(a[k], b[k], at ? at + '.' + k : k),
    );
  return a === b ? [] : [at + ': ' + JSON.stringify(a) + ' vs ' + JSON.stringify(b)];
}
const isPoll = (url) => url.pathname === '/api/model' && url.searchParams.has('since');

// A live poll that changes something: every poll is answered with the model as it would be a moment later, the oldest top-level row's
// session active just now (its `last` past every other's, so it would sort first), under a new version; a poll that asks since that
// version gets a 304. It must reach the list (that row's age reads "now") without moving any row (a reorder is held), and leave the
// rest of the page as served. At 390 on "full", opening the drawer then re-sorts: the row is first.
async function grownPoll(r, browser, which, { size, dark, P, K, reopen }) {
  const page = await embedPage(browser, which, { size, dark, saved: size === 'desktop' });
  const rows = () =>
    page.evaluate(() =>
      [...document.querySelectorAll('#lanes > .treeitem')].map((n) => ({
        id: n.dataset.id,
        ag: n.querySelector(':scope > .tree-row .srow .ag')?.textContent,
      })),
    );
  const before = await rows(),
    bumped = before.filter((x) => x.ag !== 'now').at(-1)?.id;
  let grown = null,
    served = 0;
  await page.route(isPoll, async (route) => {
    const url = new URL(route.request().url());
    if (grown && url.searchParams.get('since') === grown) return route.fulfill({ status: 304 });
    const headers = { ...route.request().headers() };
    delete headers['if-none-match'];
    url.search = '';
    const res = await route.fetch({ url: url.toString(), headers }),
      m = await res.json();
    m.sessions[bumped].last = Math.max(...Object.values(m.sessions).map((x) => x.last)) + 60000;
    m.version += '-grown';
    grown = m.version;
    served++;
    await route.fulfill({ response: res, json: m });
  });
  const drawn =
    !!bumped &&
    (await page
      .waitForFunction(
        (id) =>
          document.querySelector('#lanes .srow[data-id="' + CSS.escape(id) + '"] .ag')
            ?.textContent === 'now',
        bumped,
        { timeout: 10000 },
      )
      .then(
        () => true,
        () => false,
      ));
  await page.waitForTimeout(150);
  const after = await rows(),
    kept = await untouched(page);
  const G = (K.grown = {
    bumped,
    served,
    drawn,
    held: after.map((x) => x.id).join() === before.map((x) => x.id).join(),
    kept: kept.same,
  });
  r.expect(
    !!bumped && served >= 1 && drawn && G.held,
    P +
      ': a changed live poll did not reach the list, or moved a row: ' +
      JSON.stringify({ ...G, before: before.map((x) => x.id), after: after.map((x) => x.id) }),
  );
  r.expect(kept.same, P + ': a changed live poll changed the page: ' + JSON.stringify(kept));
  if (reopen) {
    await openDrawer(page);
    G.first = await page.evaluate(
      () => document.querySelector('#lanes > .treeitem')?.dataset.id ?? null,
    );
    r.expect(
      G.first === bumped,
      P + ': opening the drawer did not re-sort the held list: ' + JSON.stringify(G),
    );
  }
  r.expect(
    page.errors.length === 0,
    P + ': page errors on the changed poll: ' + JSON.stringify(page.errors),
  );
  await page.context().close();
}

// A row opens its session's page in the viewer by loading it: a marker set on the embedding page is gone afterwards.
async function opensRow(r, page, key, how) {
  const row = page.locator('#lanes .srow').first(),
    id = await row.getAttribute('data-id'),
    name = await row.locator('.nm').textContent();
  await page.evaluate(() => {
    window.__embedMarker = 1;
  });
  if (how === 'tap') await row.tap();
  else await row.click();
  const opened = await page
    .waitForURL((u) => u.pathname.startsWith('/s/'), { timeout: 8000 })
    .then(
      () => true,
      () => false,
    );
  const at = await page.evaluate(() => location.pathname);
  const shown =
    opened &&
    (await page
      .waitForFunction((n) => document.querySelector('#topbar .t')?.textContent === n, name, {
        timeout: 8000,
      })
      .then(
        () => true,
        () => false,
      ));
  const loaded = shown && (await page.evaluate(() => window.__embedMarker === undefined));
  r.expect(
    opened && decodeURIComponent(at.split('/').pop()) === id && shown && loaded,
    key +
      ": the row did not open its session's page by a page load: " +
      JSON.stringify({ id, name, at, shown, loaded }),
  );
  return { id, at, shown, loaded };
}

export default async function embedSidebarCheck(browser) {
  const r = reporter('embedsidebar'),
    R = r.results;
  r.expect(!!ENV.base && !!ENV.token, 'the primary fixture server is required');
  r.expect(
    BARE.includes('data-viewer="sidebar"') &&
      !/id="(main|page|topbar)"/.test(BARE) &&
      BARE.includes('<div class="sidebar-head">'),
    'the documented page is not a sidebar-only page without #main, #page and #topbar: ' +
      BARE.slice(0, 300),
  );

  const runs = [
    ['full', 'phone', false],
    ['full', 'phone', true],
    ['full', 'desktop', false],
    ['full', 'desktop', true],
    ['bare', 'phone', false],
    ['bare', 'desktop', false],
  ];
  for (const [which, size, dark] of runs) {
    const shot = (size === 'phone' ? 390 : 1280) + '-' + (dark ? 'dark' : 'light'),
      P = which + '-' + shot,
      K = (R[P] = {});
    const page = await embedPage(browser, which, { size, dark, saved: size === 'desktop' });
    const viewer = await served(browser, { size, dark, path: '/' });
    await viewer.waitForSelector('#lanes .srow', { state: 'attached' });
    const [embeddingInputs, viewerInputs] = await Promise.all([
      page.locator('#sidebar input').count(),
      viewer.locator('#sidebar input').count(),
    ]);
    K.sidebarInputs = { embedding: embeddingInputs, viewer: viewerInputs };
    r.expect(
      embeddingInputs === 0 && viewerInputs === 0,
      P + ': the sidebar contains an input: ' + JSON.stringify(K.sidebarInputs),
    );

    // The list and the navigation are the viewer's, drawn from the same model.
    const [ours, theirs] = await Promise.all([lanes(page), lanes(viewer)]);
    K.rows = ours.length;
    r.expect(ours.length > 0, P + ": the embedding page's Recent list is empty");
    r.expect(
      JSON.stringify(ours) === JSON.stringify(theirs),
      P +
        ": the Recent list differs from the viewer's: " +
        JSON.stringify({ ours: ours.map((x) => x.id), theirs: theirs.map((x) => x.id) }),
    );
    const [ourNav, theirNav] = await Promise.all([nav(page), nav(viewer)]);
    K.nav = ourNav;
    const plain = (xs) => JSON.stringify(xs.map(({ current, ...rest }) => rest));
    r.expect(
      plain(ourNav) === plain(theirNav),
      P + ": the navigation differs from the viewer's: " + plain(ourNav) + ' vs ' + plain(theirNav),
    );
    // As on the viewer's page: Home keeps its badge, Sessions has no count, and the parents' attention dots are the viewer's.
    const home = ourNav.find((x) => x.label === 'Home'),
      sessions = ourNav.find((x) => x.label === 'Sessions');
    K.badges = {
      home: home?.cnt ?? null,
      sessions: sessions?.cnt ?? null,
      flags: ours.filter((x) => x.dots.some((d) => d.startsWith('kid-flag'))).length,
    };
    r.expect(
      !!home?.cnt &&
        home.cnt === theirNav.find((x) => x.label === 'Home')?.cnt &&
        !!sessions &&
        sessions.cnt === null,
      P +
        ": the nav's badges are not the viewer's (Home's badge, no Sessions count): " +
        JSON.stringify(K.badges),
    );
    r.expect(
      ours.some((x) => x.dots.length),
      P + ': the Recent rows have no dots: ' + JSON.stringify(ours.slice(0, 2)),
    );
    r.expect(
      JSON.stringify(ourNav.map((x) => x.href)) ===
        JSON.stringify(['/', '/sessions', '/analytics', '/machines']),
      P + ': native navigation destinations differ: ' + JSON.stringify(ourNav),
    );
    r.expect(
      ourNav.every((x) => x.tag === 'A'),
      P + ': the shared renderer did not draw native navigation links: ' + JSON.stringify(ourNav),
    );
    r.expect(
      JSON.stringify(ourNav.map((x) => x.current)) === JSON.stringify([null, null, null, 'page']),
      P + ': only Machines should be current: ' + JSON.stringify(ourNav),
    );

    // Nothing outside the sidebar's own parts changed (at 1280 with a wide page and a collapsed rail saved), and there is no rail
    // and no toggle for it.
    const before = await untouched(page);
    r.expect(
      before.same,
      P + ": the viewer's script changed the embedding page: " + JSON.stringify(before),
    );
    K.rail = await page.evaluate(() => ({
      toggle: !!document.querySelector('#rail-toggle'),
      rail: document.querySelector('.app').classList.contains('rail'),
      width: document.querySelector('#sidebar').getBoundingClientRect().width,
    }));
    r.expect(
      !K.rail.toggle && !K.rail.rail,
      P + ": the embedding page has the viewer's rail or its toggle: " + JSON.stringify(K.rail),
    );
    await grownPoll(r, browser, which, {
      size,
      dark,
      P,
      K,
      reopen: which === 'full' && size === 'phone' && !dark,
    });

    if (which === 'full' && size === 'phone' && !dark) {
      // "/" belongs to the page: it opens no drawer and takes no focus.
      await page.keyboard.press('/');
      const slash = await page.evaluate(() => ({
        drawer: document.body.classList.contains('drawer-open'),
        focus: document.activeElement?.id ?? null,
      }));
      K.slash = slash;
      r.expect(
        !slash.drawer && slash.focus !== 'q',
        P + ': "/" was taken by the viewer\'s script: ' + JSON.stringify(slash),
      );
    }

    if (which === 'full') {
      if (size === 'phone') {
        await openDrawer(page);
        await openDrawer(viewer);
      }
      const [ourBox, theirBox] = await Promise.all([layout(page), layout(viewer)]);
      const diff = differences(ourBox, theirBox);
      K.layout = { ours: ourBox, differences: diff };
      r.expect(
        diff.length === 0,
        P + ": the sidebar's parts sit or read differently from the viewer's: " + diff.join('; '),
      );
      await page.screenshot({ path: path.join(OUT, 'embed-' + shot + '.png') });
      await viewer.screenshot({ path: path.join(OUT, 'viewer-' + shot + '.png') });
    }

    if (which === 'full' && size === 'phone' && !dark) {
      // From Home, "/" opens Sessions and focuses its search. Its search filters the page without narrowing the sidebar.
      await viewer.keyboard.press('/');
      const wentToSessions = await viewer
        .waitForURL((u) => u.pathname === '/sessions', { timeout: 8000 })
        .then(
          () => true,
          () => false,
        );
      const focused =
        wentToSessions &&
        (await viewer
          .waitForFunction(() => document.activeElement === document.querySelector('#sq'), null, {
            timeout: 8000,
          })
          .then(
            () => true,
            () => false,
          ));
      const homeSlash = await viewer.evaluate(() => ({
        path: location.pathname,
        focus: document.activeElement?.id ?? null,
      }));
      K.homeSlash = { ...homeSlash, focused };
      r.expect(
        wentToSessions && focused && homeSlash.path === '/sessions',
        P + ': "/" did not open Sessions with its search focused: ' + JSON.stringify(K.homeSlash),
      );

      const beforeRows = await viewer.locator('#page .nrow').count(),
        query = (await viewer.locator('#page .nrow .nm').first().textContent()).trim();
      const sideBefore = (await lanes(viewer)).map((x) => x.id);
      await viewer.fill('#sq', query);
      const afterRows = await viewer.locator('#page .nrow').count(),
        visibleNames = (await viewer.locator('#page .nrow .nm').allTextContents()).map((x) =>
          x.trim(),
        );
      const sideAfter = (await lanes(viewer)).map((x) => x.id);
      K.sessionsSearch = {
        beforeRows,
        query,
        afterRows,
        visibleNames,
        sidebarUnchanged: JSON.stringify(sideBefore) === JSON.stringify(sideAfter),
      };
      r.expect(
        beforeRows > 1 && afterRows > 0 && afterRows < beforeRows && visibleNames.includes(query),
        P + ': the Sessions search did not filter its rows: ' + JSON.stringify(K.sessionsSearch),
      );
      r.expect(
        K.sessionsSearch.sidebarUnchanged,
        P + ': the Sessions search narrowed the sidebar: ' + JSON.stringify(K.sessionsSearch),
      );

      const prefilled = await served(browser, {
        size,
        dark,
        path: '/sessions?q=' + encodeURIComponent(query),
      });
      const prefill = await prefilled.evaluate(() => ({
        value: document.querySelector('#sq')?.value ?? null,
        path: location.pathname,
        search: location.search,
      }));
      K.queryPrefill = prefill;
      r.expect(
        prefill.value === query &&
          prefill.path === '/sessions' &&
          prefill.search === '?q=' + encodeURIComponent(query),
        P + ': a direct ?q= link did not prefill the Sessions search: ' + JSON.stringify(prefill),
      );
      await prefilled.evaluate(() => document.activeElement?.blur());
      await prefilled.keyboard.press('/');
      const sessionsSlash = await prefilled.evaluate(() => ({
        path: location.pathname + location.search,
        focus: document.activeElement?.id ?? null,
      }));
      K.sessionsSlash = sessionsSlash;
      r.expect(
        sessionsSlash.path === '/sessions?q=' + encodeURIComponent(query) &&
          sessionsSlash.focus === 'sq',
        P +
          ': "/" on Sessions changed the route or failed to focus its search: ' +
          JSON.stringify(sessionsSlash),
      );

      // A modal owns focus: "/" must leave the filter sheet and its route alone.
      await prefilled.click('.facet-btn');
      await prefilled.waitForSelector('dialog.filters-sheet[open]');
      await prefilled.focus('dialog.filters-sheet .vclose');
      const modalBefore = await prefilled.evaluate(() => ({
        path: location.pathname + location.search,
        route: history.state,
        focus: document.activeElement?.className,
      }));
      await prefilled.keyboard.press('/');
      const modalAfter = await prefilled.evaluate(() => ({
        path: location.pathname + location.search,
        route: history.state,
        focus: document.activeElement?.className,
        open: !!document.querySelector('dialog.filters-sheet[open]'),
      }));
      K.modalSlash = modalAfter;
      r.expect(
        modalAfter.open &&
          modalAfter.path === modalBefore.path &&
          JSON.stringify(modalAfter.route) === JSON.stringify(modalBefore.route) &&
          modalAfter.focus === modalBefore.focus,
        P +
          ': "/" changed the route or focus behind the filter sheet: ' +
          JSON.stringify(modalAfter),
      );
      await prefilled.click('dialog.filters-sheet .vclose');
      await prefilled.waitForFunction(() => !history.state?.sheet);

      // Grouping redraws Sessions; it must preserve focus on the chosen control after the shortcut.
      await prefilled.click('.groupby button[data-g="project"]');
      const redrawFocus = await prefilled.evaluate(() => document.activeElement?.dataset.g ?? null);
      r.expect(
        redrawFocus === 'project',
        P + ': a later Sessions redraw stole focus into search: ' + redrawFocus,
      );
      r.expect(
        prefilled.errors.length === 0,
        P + ': errors on the prefilled Sessions page: ' + JSON.stringify(prefilled.errors),
      );
      await prefilled.context().close();

      // The phone's "All N" sheet opens and closes over the page without a history entry; Esc closes it and leaves the drawer open.
      // Open every collapsed top-level parent (a nested one is out of sight until its parent opens), so a long one shows "All N".
      // (One at a time: an opened toggle leaves the collapsed set, so a list taken up front would shift under the taps.)
      const collapsed = page.locator(
        '#lanes > .treeitem > .tree-row > .tree-toggle[aria-expanded="false"]',
      );
      for (let n = 0; n < 20 && (await collapsed.count()); n++) await collapsed.first().tap();
      const all = page.locator('#lanes .tree-all').first(),
        has = (await all.count()) > 0,
        length = await page.evaluate(() => history.length);
      let sheet = { has };
      if (has) {
        await all.tap();
        const open = await page.waitForSelector('dialog.kids-sheet[open]', { timeout: 4000 }).then(
          () => true,
          () => false,
        );
        const during = await page.evaluate(() => ({
          length: history.length,
          state: history.state,
        }));
        await page.keyboard.press('Escape');
        await page.waitForTimeout(300);
        const closed = await page.evaluate(() => ({
          length: history.length,
          gone: !document.querySelector('dialog.kids-sheet'),
          path: location.pathname,
          drawer: document.body.classList.contains('drawer-open'),
        }));
        sheet = { has, open, length, during, closed };
      }
      K.sheet = sheet;
      r.expect(
        has &&
          sheet.open &&
          sheet.during.length === length &&
          sheet.during.state === null &&
          sheet.closed.length === length &&
          sheet.closed.gone &&
          sheet.closed.path === page.at &&
          sheet.closed.drawer,
        P +
          ': the "All N" sheet touched the history, closed the drawer, or did not open and close: ' +
          JSON.stringify(sheet),
      );

      K.opened = await opensRow(r, page, P, 'tap');
    }

    if (which === 'bare' && size === 'desktop') {
      K.opened = await opensRow(r, page, P, 'click');
      // A refused poll stops the list's updates, with a note under the list and nothing else on the page.
      const refused = await embedPage(browser, 'bare', { size, dark, saved: true });
      await refused.route(isPoll, (route) =>
        route.fulfill({ status: 403, contentType: 'text/plain', body: 'Forbidden' }),
      );
      const noted = await refused
        .waitForSelector('#side-list .livenote-side', { timeout: 8000 })
        .then(
          () => true,
          () => false,
        );
      const kept = await untouched(refused);
      K.refused = { noted, kept: kept.same };
      r.expect(
        noted && kept.same,
        P +
          ': a refused poll left no note under the list, or changed the page: ' +
          JSON.stringify({ noted, ...kept }),
      );
      r.expect(
        refused.errors.length === 0,
        P + ': page errors on the refused page: ' + JSON.stringify(refused.errors),
      );
      await refused.context().close();
    }

    r.expect(
      page.errors.length === 0 && viewer.errors.length === 0,
      P + ': page errors: ' + JSON.stringify({ embed: page.errors, viewer: viewer.errors }),
    );
    await page.context().close();
    await viewer.context().close();
  }
  return r.done();
}

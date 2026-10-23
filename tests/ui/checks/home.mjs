// Ported from the mockup's home.js: root Sessions, the sidebar drawer, Sessions filters and Machines, and that
// Sessions rows actually lead to the reader — phone, default (light) colour scheme, plus a small desktop pass
// for two reference screenshots. The original never set colorScheme, so it only ever ran light; this port keeps
// that (bar.mjs and turns.mjs cover phone dark).
//
// The original ran 'sample' (test7.html) and 'real' (test-real.html) tagged. Ported: 'real' is dropped; 'sample'
// drives the served fixture. Every lookup here is by session id or by a live DOM query (no sample handoff/turn id
// is named), so there is no id mapping to do. Per gaps.json there is exactly one machine ('laptop') in this fixture,
// so machinePages has one entry and its screenshot is always taken (the original's condition — 'buildbox' or a
// single machine — reduces to "always" here).
//
// Assertions:
//  - no page errors.
//  - zero overflow on every screen visited: root Sessions, the drawer, filtered Sessions, Machines and every machine page.
//  - the sidebar shows the 8 most recent top-level sessions with children nested beneath their parent, and expand/collapse preferences survive reloads.
//  - every grouping on the Sessions page (project/machine/harness) still shows the same row count as ungrouped —
//    grouping reshuffles into sections, it doesn't drop or duplicate rows.
//  - a Sessions row tap opens the reader at the end; removed Home inbox content stays absent.
//    Anchored turn navigation remains covered by turns.mjs.
//  - a machine page's "up" link goes back to Machines (backTo === "Machines"), and every machine page carries no
//    sideways overflow.
import path from 'node:path';
import {
  ENV,
  served,
  data,
  reporter,
  overflow,
  settled,
  goto,
  wide,
  filterSheet,
  doneFilterSheet,
  pickFilter,
} from '../lib.mjs';

// The title is in the bar as soon as a session is clicked; the page is ready once it is no longer aria-busy.
const afterTitle = (page, text) =>
  page.waitForFunction(
    (t) =>
      document.querySelector('#topbar .t')?.textContent === t &&
      !document.querySelector('#page').hasAttribute('aria-busy') &&
      (history.state?.v !== 'session' || !!document.querySelector('#page [data-entry-key]')),
    text,
  );

export default async function homeCheck(browser) {
  const D = await data();
  const r = reporter('home');
  const qualified = (id) => JSON.stringify([D.SOURCES?.[D.SESS[id]?.machine] ?? 'local', id]);
  const spawned = new Set(D.H.filter((h) => h.kind === 'spawn').map((h) => h.to));
  const roots = Object.values(D.SESS)
    .filter((s) => s.lane && !spawned.has(s.id))
    .sort((a, b) => b.last - a.last);
  const parentOf = (sid) =>
    D.SESS[sid]?.parent ??
    D.H.find((h) => (h.kind === 'spawn' || h.kind === 'relay') && h.to === sid)?.from;
  const shownRoots = new Set(roots.slice(0, 8).map((s) => s.id));
  const treeRoot = (sid) => {
    const seen = new Set();
    while (parentOf(sid) && !seen.has(sid)) {
      seen.add(sid);
      sid = parentOf(sid);
    }
    return sid;
  };
  // A parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows are listed;
  // the rest, with everything below them, are behind "All N", so those rows aren't in the sidebar tree. A finished child with a waiting
  // or running session below it ranks as that session does.
  const kidsOf = (sid) => Object.values(D.SESS).filter((s) => s.id !== sid && s.parent === sid);
  const below = (sid, seen = new Set([sid])) =>
    kidsOf(sid).flatMap((k) => (seen.has(k.id) ? [] : (seen.add(k.id), [k, ...below(k.id, seen)])));
  const rankOf = (k) =>
    below(k.id).reduce(
      (rank, d) => (d.state === 'wait' ? 0 : d.state === 'work' ? Math.min(rank, 1) : rank),
      k.state === 'wait' ? 0 : k.state === 'work' ? 1 : 2,
    );
  const folded = new Set();
  const foldAll = (sid) => {
    if (folded.has(sid)) return;
    folded.add(sid);
    kidsOf(sid).forEach((k) => foldAll(k.id));
  };
  const foldBelow = (sid, seen = new Set()) => {
    if (seen.has(sid)) return;
    seen.add(sid);
    const kids = kidsOf(sid)
        .map((k) => ({ k, rank: rankOf(k) }))
        .sort((a, b) => a.rank - b.rank || b.k.last - a.k.last),
      keep = new Set();
    for (const { k, rank } of kids) if (rank < 2 && keep.size < 8) keep.add(k.id);
    for (const { k } of kids) if (keep.size < 3) keep.add(k.id);
    for (const { k } of kids)
      if (keep.has(k.id)) foldBelow(k.id, seen);
      else foldAll(k.id);
  };
  Object.values(D.SESS)
    .filter((s) => !s.parent)
    .forEach((s) => foldBelow(s.id));
  const expectedTreeRows = Object.values(D.SESS).filter(
    (s) => shownRoots.has(treeRoot(s.id)) && !folded.has(s.id),
  ).length;
  const treePair = D.H.find(
    (h) => h.kind === 'spawn' && roots.slice(0, 8).some((s) => s.id === h.from) && D.SESS[h.to],
  );
  const out = {};
  const errs = [];

  {
    const page = await served(browser, { size: 'phone', dark: false });
    errs.push(...page.errors);
    const over = () => overflow(page);
    const nav = async (go) => {
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.click('#lead-btn');
      await page.waitForTimeout(280);
      await page.click('.nav-item[data-go="' + go + '"]');
      await page.waitForTimeout(150);
    };

    out.root = await page.evaluate(() => ({
      title: document.querySelector('#topbar .t')?.textContent,
      path: location.pathname,
      rows: document.querySelectorAll('[data-session-row="list"]').length,
      removedInbox: document.querySelectorAll('#page .ib, #nav [data-go="home"]').length,
    }));
    out.homeOver = await over();
    await page.screenshot({ path: path.join(ENV.out, 'sample-root-sessions.png') });

    await page.click('#lead-btn');
    await page.waitForTimeout(280);
    out.sidebar = await page.evaluate(() => ({
      nav: [...document.querySelectorAll('.nav-item')].map((n) => n.textContent),
      head: document.querySelector('.side-h')?.textContent,
      treeRole: document.querySelector('#lanes')?.getAttribute('role'),
      sidebarChips: document.querySelectorAll('.sidebar .groupby button').length,
      allLink: document.querySelector('#all-sessions')?.textContent,
      gheads: document.querySelectorAll('#lanes .ghead').length,
      rows: document.querySelectorAll('#lanes .treeitem').length,
      roots: document.querySelectorAll('#lanes > .treeitem').length,
      children: document.querySelectorAll('#lanes .tree-group .treeitem').length,
      sorted: [...document.querySelectorAll('#lanes > .treeitem .srow .ag')]
        .slice(0, 5)
        .map((a) => a.textContent),
    }));
    await page.screenshot({ path: path.join(ENV.out, 'sample-drawer.png') });
    out.tree = {
      expectedRoots: Math.min(8, roots.length),
      expectedTreeRows,
      pair: treePair ? { parent: qualified(treePair.from), child: qualified(treePair.to) } : null,
    };
    if (treePair) {
      out.tree.collapse = await page.evaluate(({ parent, child }) => {
        const item = [...document.querySelectorAll('#lanes > .treeitem')].find(
            (x) => x.dataset.id === parent,
          ),
          toggle = item?.querySelector(':scope > .tree-row .tree-toggle');
        if (!item || !toggle) return null;
        if (item.getAttribute('aria-expanded') !== 'true') toggle.click();
        toggle.click();
        return {
          expanded: item.getAttribute('aria-expanded'),
          childUnderParent: [...item.querySelectorAll(':scope > .tree-group .treeitem')].some(
            (x) => x.dataset.id === child,
          ),
          stored: JSON.parse(localStorage.getItem('semon.tree') ?? '{}')[parent]?.open,
        };
      }, out.tree.pair);
      await page.reload({ waitUntil: 'load' });
      await settled(page);
      out.tree.collapseReload = await page.evaluate(({ parent, child }) => {
        const item = [...document.querySelectorAll('#lanes > .treeitem')].find(
          (x) => x.dataset.id === parent,
        );
        return {
          expanded: item?.getAttribute('aria-expanded'),
          groupDisplay: item
            ? getComputedStyle(item.querySelector(':scope > .tree-group')).display
            : null,
          childUnderParent: [
            ...(item?.querySelectorAll(':scope > .tree-group .treeitem') ?? []),
          ].some((x) => x.dataset.id === child),
          stored: JSON.parse(localStorage.getItem('semon.tree') ?? '{}')[parent]?.open,
        };
      }, out.tree.pair);
      out.tree.expand = await page.evaluate(({ parent, child }) => {
        const item = [...document.querySelectorAll('#lanes > .treeitem')].find(
            (x) => x.dataset.id === parent,
          ),
          toggle = item?.querySelector(':scope > .tree-row .tree-toggle');
        if (!item || !toggle) return null;
        if (item.getAttribute('aria-expanded') !== 'true') toggle.click();
        return {
          expanded: item.getAttribute('aria-expanded'),
          childUnderParent: [...item.querySelectorAll(':scope > .tree-group .treeitem')].some(
            (x) => x.dataset.id === child,
          ),
          stored: JSON.parse(localStorage.getItem('semon.tree') ?? '{}')[parent]?.open,
        };
      }, out.tree.pair);
      await page.reload({ waitUntil: 'load' });
      await settled(page);
      out.tree.expandReload = await page.evaluate(({ parent, child }) => {
        const item = [...document.querySelectorAll('#lanes > .treeitem')].find(
          (x) => x.dataset.id === parent,
        );
        return {
          expanded: item?.getAttribute('aria-expanded'),
          childUnderParent: [
            ...(item?.querySelectorAll(':scope > .tree-group .treeitem') ?? []),
          ].some((x) => x.dataset.id === child),
          stored: JSON.parse(localStorage.getItem('semon.tree') ?? '{}')[parent]?.open,
        };
      }, out.tree.pair);
      out.tree.keyboard = await page.evaluate(({ parent }) => {
        const item = [...document.querySelectorAll('#lanes > .treeitem')].find(
          (x) => x.dataset.id === parent,
        );
        item?.focus();
        item?.dispatchEvent(
          new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true, cancelable: true }),
        );
        const left = item?.getAttribute('aria-expanded');
        item?.dispatchEvent(
          new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true, cancelable: true }),
        );
        return { left, right: item?.getAttribute('aria-expanded') };
      }, out.tree.pair);
      await page.click('#lead-btn');
      await page.waitForTimeout(280);
      await page.evaluate(
        (id) =>
          [...document.querySelectorAll('#lanes > .treeitem .srow')]
            .find((x) => x.dataset.id === id)
            ?.click(),
        qualified(treePair.from),
      );
      await afterTitle(page, D.SESS[treePair.from].name);
      await page.waitForFunction(() => {
        const s = matchMedia('(max-width: 760px)').matches
          ? document.scrollingElement
          : document.querySelector('#main');
        return s && s.scrollHeight - s.scrollTop - s.clientHeight <= 1;
      });
      out.tree.sidebarOpensAtEnd = await page.evaluate(() => {
        const s = matchMedia('(max-width: 760px)').matches
          ? document.scrollingElement
          : document.querySelector('#main');
        return s.scrollHeight - s.scrollTop - s.clientHeight <= 1;
      });
    }
    await nav('sessions');
    out.sessionsPage = await page.evaluate(() => ({
      h1: document.querySelector('.page h1')?.textContent,
      sub: document.querySelector('.ph .sub')?.textContent,
      rows: document.querySelectorAll('.page .nrow').length,
      groupby: [...document.querySelectorAll('.page .groupby button')].map(
        (x) => x.textContent + (x.getAttribute('aria-pressed') === 'true' ? '*' : ''),
      ),
    }));
    await page.locator('[data-session-row="list"]').first().waitFor();
    out.sessionsFilters = {};
    await filterSheet(page);
    for (const name of ['Harness', 'Repo', 'Model', 'Machine']) {
      out.sessionsFilters[name] = await page
        .locator(`.filters-sheet .sh-select[data-label="${name}"] .sh-select-trigger`)
        .count();
    }
    out.sessionsFilters.search = await page
      .getByRole('searchbox', { name: 'Search session details', exact: true })
      .count();
    await doneFilterSheet(page);
    out.sessionsPage.rows = await page.locator('[data-session-row="list"]').count();
    // Browse one machine's bounded catalog; clearing metadata filters restores its rows.
    await pickFilter(page, 'Harness', 'codex');
    await page.waitForFunction(
      () =>
        location.search.includes('harness=codex') &&
        document.querySelectorAll('[data-session-row="list"]').length > 0 &&
        [...document.querySelectorAll('[data-session-row="list"]')].every((row) =>
          row.querySelector('.hicon[data-harness="codex"]'),
        ),
    );
    out.sessionsFilteredRows = await page.locator('[data-session-row="list"]').count();
    await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
    await page.waitForFunction(
      (count) => document.querySelectorAll('[data-session-row="list"]').length === count,
      out.sessionsPage.rows,
    );
    out.sessionsRestoredRows = await page.locator('[data-session-row="list"]').count();
    out.sessionsOver = await over();
    await nav('machines');
    out.machines = await page.evaluate(() => ({
      h1: document.querySelector('.page h1')?.textContent,
      sub: document.querySelector('.ph .sub')?.textContent,
      rows: [...document.querySelectorAll('.page .nrow')].map((r) =>
        r.innerText.replace(/\n/g, ' / '),
      ),
    }));
    out.machinesOver = await over();
    await page.screenshot({ path: path.join(ENV.out, 'sample-machines.png') });
    const ms = await page.evaluate(() =>
      [...document.querySelectorAll('.page .nrow')].map((r) => r.dataset.m),
    );
    r.expect(ms.length > 0, 'no machine rows found on the Machines page');
    out.machinePages = [];
    for (const m of ms) {
      await nav('machines');
      await page.click('.nrow[data-m="' + m + '"]');
      const machineId = Object.keys(D.MACHINE).find((id) => (D.SOURCES?.[id] ?? id) === m);
      const machineName = D.MACHINE[machineId ?? m];
      r.expect(!!machineName, 'the selected source has an independently observed machine name');
      await afterTitle(page, machineName);
      await page.locator('#page .nrow[data-id]').first().waitFor();
      const info = await page.evaluate(() => ({
        h1: document.querySelector('#topbar .t')?.textContent,
        rows: document.querySelectorAll('#page .nrow[data-id]').length,
        source: new URLSearchParams(location.search).get('machine'),
        up: document.querySelector('.uplink')?.textContent,
        crumb: document.querySelector('.topbar .crumb')?.textContent,
        cur: document.querySelector('.nav-item[aria-current="page"]')?.textContent,
        secs: [...document.querySelectorAll('.page .sec-h')].map((s) => s.textContent),
        sub: document.querySelector('.topbar .meta-line')?.textContent,
        moves: [...document.querySelectorAll('.page .ib')].map(
          (x) => x.querySelector('.org')?.textContent.slice(0, 60) ?? null,
        ),
      }));
      info.expectedName = machineName;
      info.expectedSource = m;
      info.over = await over();
      out.machinePages.push(info);
      await page.screenshot({
        path: path.join(ENV.out, 'sample-machine-' + m + '.png'),
        fullPage: true,
      });
      await page.goBack();
      await page.waitForTimeout(150);
      info.backTo = await page.evaluate(() => document.querySelector('.page h1')?.textContent);
    }
    // Sessions rows open the retained reader at its end.
    await nav('sessions');
    const first = await page.$('.page .nrow');
    r.expect(!!first, 'no session row (.page .nrow) found on Sessions');
    if (first) {
      await first.click();
      await page.waitForFunction(
        () =>
          history.state?.v === 'session' &&
          !!document.querySelector("#page section[aria-label='Transcript']"),
      );
      out.liveRowOpens = await page.evaluate(() => {
        const s = matchMedia('(max-width: 760px)').matches
          ? document.scrollingElement
          : document.querySelector('#main');
        return {
          title: document.querySelector('#topbar .t')?.textContent,
          gap: s.scrollHeight - s.scrollTop - s.clientHeight,
        };
      });
    }
    // Needs-you cards belonged only to the removed Home inbox. Retain row
    // opening/end and history checks here; turns.mjs covers anchored links.
    await nav('sessions');
    out.removedInbox = await page.locator('#page .ib, #nav [data-go="home"]').count();
    errs.push(...page.errors);
    await page.context().close();
  }

  // Desktop, light: two reference screenshots.
  {
    const page = await served(browser, { size: 'desktop', dark: false });
    // Wide is set from a session's menu (it only ever widens a transcript); the rail from the sidebar. Both survive a reload.
    await goto(page, { v: 'session', id: Object.keys(D.SESS)[0] }, D);
    await wide(page, true);
    await page.click('#rail-toggle');
    const wideSid = Object.keys(D.SESS)[0];
    await page.reload({ waitUntil: 'load' });
    await settled(page);
    await goto(page, { v: 'session', id: wideSid }, D);
    out.layout = await page.evaluate(() => ({
      wide: document.querySelector('#page').classList.contains('wide-mode'),
      rail: document.querySelector('.app').classList.contains('rail'),
      wideStored: localStorage.getItem('semon.wide'),
      railStored: localStorage.getItem('semon.rail'),
    }));
    await wide(page, false);
    await page.click('#rail-toggle');
    await page.reload({ waitUntil: 'load' });
    await settled(page);
    await goto(page, { v: 'session', id: wideSid }, D);
    out.layout.offReload = await page.evaluate(() => ({
      wide: document.querySelector('#page').classList.contains('wide-mode'),
      rail: document.querySelector('.app').classList.contains('rail'),
      wideStored: localStorage.getItem('semon.wide'),
      railStored: localStorage.getItem('semon.rail'),
    }));
    await page.screenshot({ path: path.join(ENV.out, 'desk-session.png') });
    const longSid = Object.keys(D.TX).sort(
      (a, b) => (D.TX[b]?.length ?? 0) - (D.TX[a]?.length ?? 0),
    )[0];
    await page.setViewportSize({ width: 1280, height: 420 });
    await page.click('.nav-item[data-go="sessions"]');
    await afterTitle(page, 'Sessions');
    out.historyScroll = {
      listBefore: await page.evaluate(() => {
        const s = document.querySelector('#main');
        s.scrollTop = Math.min(120, s.scrollHeight - s.clientHeight);
        return s.scrollTop;
      }),
    };
    await page.waitForTimeout(100);
    await page.evaluate(
      (id) =>
        [...document.querySelectorAll('.page .nrow')].find((x) => x.dataset.id === id)?.click(),
      longSid,
    );
    await afterTitle(page, D.SESS[longSid].name);
    out.historyScroll.openEnd = await page.evaluate(() => {
      const s = document.querySelector('#main');
      return s.scrollHeight - s.scrollTop - s.clientHeight;
    });
    const historyTravel = await page.evaluate(() => {
      const s = document.querySelector('#main');
      return s.scrollHeight - s.clientHeight;
    });
    await page.mouse.move(640, 200);
    await page.mouse.wheel(0, -Math.round(historyTravel * 0.55));
    await page.waitForFunction(() => {
      const s = document.querySelector('#main');
      return s.scrollTop < s.scrollHeight - s.clientHeight - 2;
    });
    await page.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    );
    out.historyScroll.readerAt = await page.evaluate(() => {
      const s = document.querySelector('#main');
      return { top: s.scrollTop, max: s.scrollHeight - s.clientHeight };
    });
    await page.goBack();
    await afterTitle(page, 'Sessions');
    await page.waitForTimeout(80);
    out.historyScroll.backTop = await page.evaluate(
      () => document.querySelector('#main').scrollTop,
    );
    await page.goForward();
    await afterTitle(page, D.SESS[longSid].name);
    await page.waitForTimeout(80);
    out.historyScroll.forwardTop = await page.evaluate(
      () => document.querySelector('#main').scrollTop,
    );
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.click('.nav-item[data-go="machines"]');
    await afterTitle(page, 'Machines');
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(ENV.out, 'desk-machines.png') });
    errs.push(...page.errors);
    await page.context().close();
  }

  r.results = out;
  r.expect(errs.length === 0, 'page errors: ' + errs.join(' | '));
  r.expect(out.homeOver === 0, 'root Sessions overflow=' + out.homeOver);
  r.expect(out.root.title === 'Sessions' && out.root.path === '/sessions' && out.root.rows > 0 && out.root.removedInbox === 0, 'root retains the existing Sessions list without Home');
  r.expect(out.sessionsOver === 0, 'sessions overflow=' + out.sessionsOver);
  r.expect(out.machinesOver === 0, 'machines overflow=' + out.machinesOver);
  for (const mp of out.machinePages)
    r.expect(mp.over === 0, 'machine page overflow=' + mp.over + ' (' + mp.h1 + ')');
  r.expect(
    out.sidebar.roots === out.tree.expectedRoots,
    'sidebar top-level rows=' +
      out.sidebar.roots +
      ' expected up to 8, got ' +
      out.tree.expectedRoots,
  );
  r.expect(
    out.sidebar.rows === out.tree.expectedTreeRows &&
      out.sidebar.children === out.tree.expectedTreeRows - out.tree.expectedRoots,
    'sidebar tree does not render the eight most recent top-level sessions with all children nested: ' +
      JSON.stringify({
        sidebar: out.sidebar,
        expectedTreeRows: out.tree.expectedTreeRows,
        expectedRoots: out.tree.expectedRoots,
      }),
  );
  r.expect(out.sidebar.treeRole === 'tree', 'sidebar session rows are missing their tree role');
  r.expect(
    out.sidebar.children > 0 && !!out.tree.pair,
    'sidebar tree has no child session under a parent',
  );
  if (out.tree.pair) {
    r.expect(
      out.tree.collapse?.expanded === 'false' &&
        out.tree.collapse.childUnderParent &&
        out.tree.collapse.stored === false,
      'tree collapse was not saved: ' + JSON.stringify(out.tree.collapse),
    );
    r.expect(
      out.tree.collapseReload?.expanded === 'false' &&
        out.tree.collapseReload.groupDisplay === 'none' &&
        out.tree.collapseReload.childUnderParent &&
        out.tree.collapseReload.stored === false,
      'tree collapse did not survive reload: ' + JSON.stringify(out.tree.collapseReload),
    );
    r.expect(
      out.tree.expand?.expanded === 'true' &&
        out.tree.expand.childUnderParent &&
        out.tree.expand.stored === true,
      'tree expand did not show its child and save: ' + JSON.stringify(out.tree.expand),
    );
    r.expect(
      out.tree.expandReload?.expanded === 'true' &&
        out.tree.expandReload.childUnderParent &&
        out.tree.expandReload.stored === true,
      'tree expansion did not survive reload: ' + JSON.stringify(out.tree.expandReload),
    );
    r.expect(
      out.tree.keyboard?.left === 'false' && out.tree.keyboard.right === 'true',
      'tree arrow keys did not collapse and expand: ' + JSON.stringify(out.tree.keyboard),
    );
    r.expect(
      out.tree.sidebarOpensAtEnd === true,
      'opening a sidebar session did not land at the end',
    );
  }
  r.expect(
    out.sessionsPage.rows > 0 && out.sessionsPage.rows <= 60,
    'the selected catalog page must be bounded',
  );
  r.expect(
    Object.values(out.sessionsFilters).every((count) => count === 1),
    'the selected catalog must expose each metadata filter once',
  );
  r.expect(
    out.sessionsFilteredRows > 0 &&
      out.sessionsFilteredRows < out.sessionsPage.rows &&
      out.sessionsRestoredRows === out.sessionsPage.rows,
    'metadata filtering and clearing must preserve the selected catalog',
  );
  for (const mp of out.machinePages)
    r.expect(
      mp.h1 === mp.expectedName &&
        mp.source === mp.expectedSource &&
        mp.crumb === 'Machines' &&
        mp.rows > 0 &&
        mp.rows <= 60,
      'a machine opens bounded native activity under its exact source and name',
    );
  r.expect(
    !!out.liveRowOpens && out.liveRowOpens.title !== 'Sessions' && out.liveRowOpens.gap <= 1,
    'a Sessions row did not open at the end: ' + JSON.stringify(out.liveRowOpens),
  );
  r.expect(out.removedInbox === 0, 'Home inbox/navigation must not move into Sessions');
  r.expect(
    out.layout?.wide === true &&
      out.layout.rail === true &&
      out.layout.wideStored === '1' &&
      out.layout.railStored === '1',
    'wide or rail mode did not survive reload: ' + JSON.stringify(out.layout),
  );
  r.expect(
    out.layout?.offReload?.wide === false &&
      out.layout.offReload.rail === false &&
      out.layout.offReload.wideStored === '0' &&
      out.layout.offReload.railStored === '0',
    'wide or rail mode off state did not survive reload: ' + JSON.stringify(out.layout?.offReload),
  );
  r.expect(
    out.historyScroll?.openEnd <= 1,
    'opening a Sessions row did not land at the end: ' + JSON.stringify(out.historyScroll),
  );
  r.expect(
    out.historyScroll?.readerAt?.max > 10 &&
      Math.abs(out.historyScroll.backTop - out.historyScroll.listBefore) <= 2 &&
      Math.abs(out.historyScroll.forwardTop - out.historyScroll.readerAt.top) <= 2,
    'Back or Forward did not restore the reader position: ' + JSON.stringify(out.historyScroll),
  );
  for (const mp of out.machinePages)
    r.expect(
      mp.backTo === 'Machines',
      'machine page back did not return to Machines: ' + mp.backTo,
    );

  return r.done();
}

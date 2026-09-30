// The viewer's sidebar on an embedding page (docs/shell.md, "The viewer's sidebar on an embedding page"). Two pages of their own load
// /shell.js and then /viewer.js with their .app marked data-viewer="sidebar", each served from the primary fixture's origin with the
// viewer's own headers (CSP included), as shell.mjs serves the gallery:
//   - "full", tests/ui/shell-sidebar.html: drawn with shell::session_sidebar (a Rust test holds it to the API), with the signed-in
//     skeleton's top bar, #main and #page;
//   - "bare", the page docs/shell.md gives as the whole of such a page, with its comment replaced by the same sidebar markup: no
//     #main, #page or #topbar.
// On both, at 390 and 1280:
//   - the viewer's script draws the navigation (its buttons, with Home's badge and no Sessions count, as the viewer's page has) and the
//     Recent list from /api/model: the same rows, in the same order, with the same text and dots (state, and a parent's attention
//     dot), as the viewer's own page, and only Machines is current;
//   - nothing outside the sidebar's own parts (header, search, nav, Recent list) changes: the document, serialized without those
//     parts, equals the page as served (so the body gets no new children and no element outside them a class, attribute or child),
//     the address is the page's, and no page error is thrown; the same holds after a live poll.
// On "full", light and dark: the search, nav, Recent label and first row sit where the viewer's do (on a phone, in the open drawer),
// with the same type, and screenshots of both go to out/embedsidebar/. At 390 light: "/" opens no drawer and takes no focus; the
// phone's "All N" sheet opens and closes (Esc, which leaves the drawer open) without a history entry; the search narrows the list as
// the viewer's does; a tap on a row opens its session's page in the viewer by a full page load (a marker set on the page is gone).
// On "bare" at 1280: a click on a row does the same; a refused poll (403) leaves a note under the list and nothing else; Enter in the
// search opens the viewer's Sessions page, by a page load, with the query in its search field.
import fs from "node:fs";
import path from "node:path";
import { ENV, context, served, reporter } from "../lib.mjs";

const OUT = path.join(ENV.out, "embedsidebar");
fs.mkdirSync(OUT, { recursive: true });
const FULL = fs.readFileSync(new URL("../shell-sidebar.html", import.meta.url), "utf8");
// The sidebar markup the full page draws with session_sidebar, put where the documented page has its comment.
const SIDEBAR = FULL.split('<aside class="sidebar" id="sidebar" aria-label="Navigation">\n')[1].split('\n<div class="account">')[0];
const DOC = fs.readFileSync(new URL("../../../docs/shell.md", import.meta.url), "utf8");
const BARE = DOC.split("## The viewer's sidebar on an embedding page\n")[1].split("```html\n")[1].split("```")[0]
  .replace('<!-- session_sidebar("Semon", nav) -->', SIDEBAR);
const PAGES = { full: ["/__shell-sidebar.html", FULL], bare: ["/__shell-sidebar-bare.html", BARE] };

async function embedPage(browser, which, { size, dark, wide = false }) {
  const [at, html] = PAGES[which];
  const ctx = await context(browser, { size, dark });
  if (wide) await ctx.addInitScript(() => { try { localStorage.setItem("semon.wide", "1"); } catch {} });
  const page = await ctx.newPage();
  page.errors = [];
  page.on("pageerror", (e) => page.errors.push(e.message.split("\n")[0]));
  page.setDefaultTimeout(8000);
  await page.clock.setFixedTime(ENV.now);
  await page.route(/.*/, (r) => (r.request().url().startsWith(ENV.base + "/") ? r.continue() : r.abort()));
  // Loading a stylesheet with the token sets the server's cookie and gives the headers the viewer's origin sends with a page.
  const sheet = await page.goto(ENV.base + "/shell.css?t=" + ENV.token, { waitUntil: "load" });
  const headers = Object.fromEntries(Object.entries(sheet.headers())
    .filter(([name]) => !["content-type", "content-length", "content-encoding", "etag", "set-cookie", "date", "connection", "transfer-encoding", "keep-alive"].includes(name)));
  await page.route(ENV.base + at, (route) => route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", headers, body: html }));
  await page.goto(ENV.base + at, { waitUntil: "load" });
  await page.waitForSelector("#lanes .srow", { state: "attached" });
  await page.evaluate(() => document.fonts.ready);
  page.at = at; page.html = html;
  return page;
}

// The document serialized with the sidebar's own parts (what the viewer's script draws into) cut out, now and as served. Any other
// change, a new child of the body, a class or attribute on the page's elements, shows as a difference.
const untouched = (page) => page.evaluate(({ html, at }) => {
  const OWN = [".sidebar-head", ".side-search", "#nav", ".side-h", "#side-list"];
  const text = (root) => { const c = root.cloneNode(true); for (const sel of OWN) c.querySelectorAll(sel).forEach((n) => n.replaceWith(document.createComment(sel))); return c.outerHTML; };
  const was = text(new DOMParser().parseFromString(html, "text/html").documentElement), now = text(document.documentElement);
  let i = 0; while (i < was.length && was[i] === now[i]) i++;
  return { same: was === now && location.pathname === at, path: location.pathname, at: i, was: was.slice(Math.max(0, i - 80), i + 120), now: now.slice(Math.max(0, i - 80), i + 120) };
}, { html: page.html, at: page.at });

const openDrawer = async (page) => { await page.click("#lead-btn"); await page.waitForTimeout(350); };
// Each row: its id, label and text, and its dots (the state dot, and a parent's attention dot, amber or red), by class.
const lanes = (page) => page.evaluate(() => [...document.querySelectorAll("#lanes .srow")].map((r) => ({ id: r.dataset.id, label: r.getAttribute("aria-label"), text: r.textContent,
  dots: [...r.querySelectorAll(".dot, .kid-flag")].map((d) => d.className) })));
const nav = (page) => page.evaluate(() => [...document.querySelectorAll("#nav .nav-item")].map((b) => ({
  tag: b.tagName, label: b.querySelector(":scope > span:not(.cnt)")?.textContent ?? null, cnt: b.querySelector(".cnt")?.textContent ?? null,
  hot: !!b.querySelector(".cnt.hot"), current: b.getAttribute("aria-current"),
})));
// Where the sidebar's parts sit in it, and their type. The list's own height is left out: the embedding page's account row under it
// takes room the viewer's page (no account on this fixture) does not.
const layout = (page) => page.evaluate(() => {
  const side = document.querySelector("#sidebar").getBoundingClientRect();
  const box = (sel) => { const e = document.querySelector(sel); if (!e) return null; const b = e.getBoundingClientRect(); return { left: b.left - side.left, top: b.top - side.top, width: b.width, height: b.height }; };
  const type = (sel) => { const e = document.querySelector(sel); if (!e) return null; const s = getComputedStyle(e); return { size: s.fontSize, weight: s.fontWeight, family: s.fontFamily, color: s.color }; };
  const list = box("#side-list");
  return {
    sidebar: { width: side.width }, search: box(".side-search"), nav: box("#nav"), navRow: box("#nav .nav-item"), recent: box(".side-h"),
    list: list && { left: list.left, top: list.top, width: list.width }, row: box("#lanes .srow"), rowName: type("#lanes .srow .nm"),
    rowMeta: type("#lanes .srow-meta"), recentType: type(".side-h"), badge: type("#nav .cnt"),
  };
});
function differences(a, b, at = "") {
  if (typeof a === "number" && typeof b === "number") return Math.abs(a - b) <= 0.5 ? [] : [at + ": " + a + " vs " + b];
  if (a && b && typeof a === "object" && typeof b === "object") return [...new Set([...Object.keys(a), ...Object.keys(b)])].flatMap((k) => differences(a[k], b[k], at ? at + "." + k : k));
  return a === b ? [] : [at + ": " + JSON.stringify(a) + " vs " + JSON.stringify(b)];
}
const nextPoll = (page) => page.waitForResponse((res) => /\/api\/model\?since=/.test(res.url()), { timeout: 8000 }).then(() => true, () => false);
const isPoll = (url) => url.pathname === "/api/model" && url.searchParams.has("since");

// A row opens its session's page in the viewer by loading it: a marker set on the embedding page is gone afterwards.
async function opensRow(r, page, key, how) {
  const row = page.locator("#lanes .srow").first(), id = await row.getAttribute("data-id"), name = await row.locator(".nm").textContent();
  await page.evaluate(() => { window.__embedMarker = 1; });
  if (how === "tap") await row.tap(); else await row.click();
  const opened = await page.waitForURL((u) => u.pathname.startsWith("/s/"), { timeout: 8000 }).then(() => true, () => false);
  const at = await page.evaluate(() => location.pathname);
  const shown = opened && await page.waitForFunction((n) => document.querySelector("#topbar .t")?.textContent === n, name, { timeout: 8000 }).then(() => true, () => false);
  const loaded = shown && await page.evaluate(() => window.__embedMarker === undefined);
  r.expect(opened && decodeURIComponent(at.split("/").pop()) === id && shown && loaded, key + ": the row did not open its session's page by a page load: " + JSON.stringify({ id, name, at, shown, loaded }));
  return { id, at, shown, loaded };
}

export default async function embedSidebarCheck(browser) {
  const r = reporter("embedsidebar"), R = r.results;
  r.expect(!!ENV.base && !!ENV.token, "the primary fixture server is required");
  r.expect(BARE.includes('data-viewer="sidebar"') && !/id="(main|page|topbar)"/.test(BARE) && BARE.includes('<div class="sidebar-head">'), "the documented page is not a sidebar-only page without #main, #page and #topbar: " + BARE.slice(0, 300));

  const runs = [["full", "phone", false], ["full", "phone", true], ["full", "desktop", false], ["full", "desktop", true], ["bare", "phone", false], ["bare", "desktop", false]];
  for (const [which, size, dark] of runs) {
    const shot = (size === "phone" ? 390 : 1280) + "-" + (dark ? "dark" : "light"), P = which + "-" + shot, K = (R[P] = {});
    const page = await embedPage(browser, which, { size, dark, wide: size === "desktop" });
    const viewer = await served(browser, { size, dark, path: "/" });
    await viewer.waitForSelector("#lanes .srow", { state: "attached" });

    // The list and the navigation are the viewer's, drawn from the same model.
    const [ours, theirs] = await Promise.all([lanes(page), lanes(viewer)]);
    K.rows = ours.length;
    r.expect(ours.length > 0, P + ": the embedding page's Recent list is empty");
    r.expect(JSON.stringify(ours) === JSON.stringify(theirs), P + ": the Recent list differs from the viewer's: " + JSON.stringify({ ours: ours.map((x) => x.id), theirs: theirs.map((x) => x.id) }));
    const [ourNav, theirNav] = await Promise.all([nav(page), nav(viewer)]);
    K.nav = ourNav;
    const plain = (xs) => JSON.stringify(xs.map(({ current, ...rest }) => rest));
    r.expect(plain(ourNav) === plain(theirNav), P + ": the navigation differs from the viewer's: " + plain(ourNav) + " vs " + plain(theirNav));
    // As on the viewer's page: Home keeps its badge, Sessions has no count, and the parents' attention dots are the viewer's.
    const home = ourNav.find((x) => x.label === "Home"), sessions = ourNav.find((x) => x.label === "Sessions");
    K.badges = { home: home?.cnt ?? null, sessions: sessions?.cnt ?? null, flags: ours.filter((x) => x.dots.some((d) => d.startsWith("kid-flag"))).length };
    r.expect(!!home?.cnt && home.cnt === theirNav.find((x) => x.label === "Home")?.cnt && !!sessions && sessions.cnt === null, P + ": the nav's badges are not the viewer's (Home's badge, no Sessions count): " + JSON.stringify(K.badges));
    r.expect(ours.some((x) => x.dots.length), P + ": the Recent rows have no dots: " + JSON.stringify(ours.slice(0, 2)));
    r.expect(ourNav.every((x) => x.tag === "BUTTON"), P + ": the viewer's script did not draw its navigation: " + JSON.stringify(ourNav));
    r.expect(JSON.stringify(ourNav.map((x) => x.current)) === JSON.stringify([null, null, null, "page"]), P + ": only Machines should be current: " + JSON.stringify(ourNav));

    // Nothing outside the sidebar's own parts changed, before and after a live poll.
    const before = await untouched(page);
    r.expect(before.same, P + ": the viewer's script changed the embedding page: " + JSON.stringify(before));
    K.polled = await nextPoll(page);
    await page.waitForTimeout(150);
    const after = await untouched(page);
    r.expect(K.polled && after.same && (await page.locator("#lanes .srow").count()) === ours.length, P + ": a live poll changed the page or lost the list: " + JSON.stringify({ polled: K.polled, ...after }));

    if (which === "full" && size === "phone" && !dark) {
      // "/" belongs to the page: it opens no drawer and takes no focus.
      await page.keyboard.press("/");
      const slash = await page.evaluate(() => ({ drawer: document.body.classList.contains("drawer-open"), focus: document.activeElement?.id ?? null }));
      K.slash = slash;
      r.expect(!slash.drawer && slash.focus !== "q", P + ": \"/\" was taken by the viewer's script: " + JSON.stringify(slash));
    }

    if (which === "full") {
      if (size === "phone") { await openDrawer(page); await openDrawer(viewer); }
      const [ourBox, theirBox] = await Promise.all([layout(page), layout(viewer)]);
      const diff = differences(ourBox, theirBox);
      K.layout = { ours: ourBox, differences: diff };
      r.expect(diff.length === 0, P + ": the sidebar's parts sit or read differently from the viewer's: " + diff.join("; "));
      await page.screenshot({ path: path.join(OUT, "embed-" + shot + ".png") });
      await viewer.screenshot({ path: path.join(OUT, "viewer-" + shot + ".png") });
    }

    if (which === "full" && size === "phone" && !dark) {
      // The phone's "All N" sheet opens and closes over the page without a history entry; Esc closes it and leaves the drawer open.
      // Open every collapsed top-level parent (a nested one is out of sight until its parent opens), so a long one shows "All N".
      for (const t of await page.locator('#lanes > .treeitem > .tree-row > .tree-toggle[aria-expanded="false"]').all()) await t.tap();
      const all = page.locator("#lanes .tree-all").first(), has = (await all.count()) > 0, length = await page.evaluate(() => history.length);
      let sheet = { has };
      if (has) {
        await all.tap();
        const open = await page.waitForSelector("dialog.kids-sheet[open]", { timeout: 4000 }).then(() => true, () => false);
        const during = await page.evaluate(() => ({ length: history.length, state: history.state }));
        await page.keyboard.press("Escape"); await page.waitForTimeout(300);
        const closed = await page.evaluate(() => ({ length: history.length, gone: !document.querySelector("dialog.kids-sheet"), path: location.pathname, drawer: document.body.classList.contains("drawer-open") }));
        sheet = { has, open, length, during, closed };
      }
      K.sheet = sheet;
      r.expect(has && sheet.open && sheet.during.length === length && sheet.during.state === null && sheet.closed.length === length && sheet.closed.gone && sheet.closed.path === page.at && sheet.closed.drawer,
        P + ": the \"All N\" sheet touched the history, closed the drawer, or did not open and close: " + JSON.stringify(sheet));

      // The search narrows the list as the viewer's does.
      const query = theirs[theirs.length - 1].text.slice(0, 4).trim();
      await page.fill("#q", query); await viewer.fill("#q", query);
      const [ourFound, theirFound] = await Promise.all([lanes(page), lanes(viewer)]);
      K.search = { query, rows: ourFound.length };
      r.expect(ourFound.length > 0 && JSON.stringify(ourFound.map((x) => x.id)) === JSON.stringify(theirFound.map((x) => x.id)), P + ": the search narrows the list differently: " + JSON.stringify({ query, ours: ourFound.map((x) => x.id), theirs: theirFound.map((x) => x.id) }));
      await page.fill("#q", "");
      K.opened = await opensRow(r, page, P, "tap");
    }

    if (which === "bare" && size === "desktop") {
      K.opened = await opensRow(r, page, P, "click");
      // A refused poll stops the list's updates, with a note under the list and nothing else on the page.
      const refused = await embedPage(browser, "bare", { size, dark });
      await refused.route(isPoll, (route) => route.fulfill({ status: 403, contentType: "text/plain", body: "Forbidden" }));
      const noted = await refused.waitForSelector("#side-list .livenote-side", { timeout: 8000 }).then(() => true, () => false);
      const kept = await untouched(refused);
      K.refused = { noted, kept: kept.same };
      r.expect(noted && kept.same, P + ": a refused poll left no note under the list, or changed the page: " + JSON.stringify({ noted, ...kept }));
      // Enter in the search opens the viewer's Sessions page, searching for the same text.
      await refused.evaluate(() => { window.__embedMarker = 1; });
      const query = ours[0].text.slice(0, 4).trim();
      await refused.fill("#q", query); await refused.locator("#q").press("Enter");
      const went = await refused.waitForURL((u) => u.pathname === "/sessions", { timeout: 8000 }).then(() => true, () => false);
      const field = went ? await refused.waitForSelector("#sq", { timeout: 8000 }).then(() => refused.evaluate(() => ({ value: document.querySelector("#sq").value, marker: window.__embedMarker ?? null })), () => null) : null;
      K.enter = { query, went, field };
      r.expect(went && field?.value === query && field.marker === null, P + ": Enter did not open the Sessions page with the search: " + JSON.stringify(K.enter));
      r.expect(refused.errors.length === 0, P + ": page errors on the refused page: " + JSON.stringify(refused.errors));
      await refused.context().close();
    }

    r.expect(page.errors.length === 0 && viewer.errors.length === 0, P + ": page errors: " + JSON.stringify({ embed: page.errors, viewer: viewer.errors }));
    await page.context().close();
    await viewer.context().close();
  }
  return r.done();
}

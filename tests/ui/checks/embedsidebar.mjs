// The viewer's sidebar on an embedding page (docs/shell.md, "The viewer's sidebar on an embedding page"): a page of its own,
// tests/ui/shell-sidebar.html, drawn with shell::session_sidebar and marked data-viewer="sidebar", loads /shell.js and then
// /viewer.js. It is served from the primary fixture's origin with the viewer's own headers (CSP included), as shell.mjs serves the
// gallery. At 390 and 1280, light and dark:
//   - the viewer's script draws the navigation (its buttons, with the Sessions count pill and every other pill the viewer's page
//     has) and the Recent list from /api/model: the same rows, in the same order, with the same text, as the viewer's own page;
//   - the row the page names (data-viewer-nav="machines") is the only current one;
//   - the page stays the embedding page's: its own content and top bar are untouched, its address is not rewritten, a saved wide
//     page does not widen it, and a live poll redraws the list without touching the page;
//   - the search field, the Recent label, the nav and the first row sit where the viewer's do in its sidebar (on a phone, in the
//     open drawer), with the same type;
//   - the search narrows the list as the viewer's does;
//   - a tap (phone) or a click (desktop) on a row opens that session's page in the viewer.
// No page errors on either page. Screenshots of both sidebars (on a phone, the open drawer) go to out/embedsidebar/.
import fs from "node:fs";
import path from "node:path";
import { ENV, context, served, reporter } from "../lib.mjs";

const OUT = path.join(ENV.out, "embedsidebar");
fs.mkdirSync(OUT, { recursive: true });
const HTML = fs.readFileSync(new URL("../shell-sidebar.html", import.meta.url), "utf8");
const PATH = "/__shell-sidebar.html";

async function embedPage(browser, { size, dark, wide = false }) {
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
  await page.route(ENV.base + PATH, (route) => route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", headers, body: HTML }));
  await page.goto(ENV.base + PATH, { waitUntil: "load" });
  await page.waitForSelector("#lanes .srow", { state: "attached" });
  await page.evaluate(() => document.fonts.ready);
  return page;
}

const openDrawer = async (page) => { await page.click("#lead-btn"); await page.waitForTimeout(350); };

const lanes = (page) => page.evaluate(() => [...document.querySelectorAll("#lanes .srow")].map((r) => ({ id: r.dataset.id, label: r.getAttribute("aria-label"), text: r.textContent })));
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
    rowMeta: type("#lanes .srow-meta"), recentType: type(".side-h"), pill: type("#nav .cnt"),
  };
});
function differences(a, b, at = "") {
  if (typeof a === "number" && typeof b === "number") return Math.abs(a - b) <= 0.5 ? [] : [at + ": " + a + " vs " + b];
  if (a && b && typeof a === "object" && typeof b === "object") return [...new Set([...Object.keys(a), ...Object.keys(b)])].flatMap((k) => differences(a[k], b[k], at ? at + "." + k : k));
  return a === b ? [] : [at + ": " + JSON.stringify(a) + " vs " + JSON.stringify(b)];
}

export default async function embedSidebarCheck(browser) {
  const r = reporter("embedsidebar"), R = r.results;
  r.expect(!!ENV.base && !!ENV.token, "the primary fixture server is required");

  for (const [size, width] of [["phone", 390], ["desktop", 1280]]) {
    for (const dark of [false, true]) {
      const key = width + "-" + (dark ? "dark" : "light"), K = (R[key] = {});
      const page = await embedPage(browser, { size, dark, wide: size === "desktop" });
      const viewer = await served(browser, { size, dark, path: "/" });
      await viewer.waitForSelector("#lanes .srow", { state: "attached" });

      // The list and the navigation are the viewer's, drawn from the same model.
      const [ours, theirs] = await Promise.all([lanes(page), lanes(viewer)]);
      K.rows = ours.length;
      r.expect(ours.length > 0, key + ": the embedding page's Recent list is empty");
      r.expect(JSON.stringify(ours) === JSON.stringify(theirs), key + ": the Recent list differs from the viewer's: " + JSON.stringify({ ours: ours.map((x) => x.id), theirs: theirs.map((x) => x.id) }));
      const [ourNav, theirNav] = await Promise.all([nav(page), nav(viewer)]);
      K.nav = ourNav;
      const plain = (xs) => JSON.stringify(xs.map(({ current, ...rest }) => rest));
      r.expect(plain(ourNav) === plain(theirNav), key + ": the navigation differs from the viewer's: " + plain(ourNav) + " vs " + plain(theirNav));
      const pill = ourNav.find((x) => x.label === "Sessions")?.cnt;
      r.expect(!!pill && pill === theirNav.find((x) => x.label === "Sessions")?.cnt, key + ": the Sessions count pill is missing or not the viewer's: " + JSON.stringify({ ours: ourNav, theirs: theirNav }));
      r.expect(ourNav.every((x) => x.tag === "BUTTON"), key + ": the viewer's script did not draw its navigation: " + JSON.stringify(ourNav));
      r.expect(JSON.stringify(ourNav.map((x) => x.current)) === JSON.stringify([null, null, null, "page"]), key + ": only Machines should be current: " + JSON.stringify(ourNav));

      // The page is still the embedding page's.
      const own = await page.evaluate(() => ({ path: location.pathname, content: !!document.querySelector("#page > #own-content"), children: document.querySelector("#page").childElementCount,
        title: document.querySelector("#topbar .ttl .t")?.textContent, bar: document.querySelector("#topbar").childElementCount, wide: document.querySelector("#page").classList.contains("wide-mode") }));
      K.own = own;
      r.expect(own.path === "/__shell-sidebar.html" && own.content && own.children === 2 && own.title === "Devices" && own.bar === 2 && !own.wide, key + ": the viewer's script changed the embedding page: " + JSON.stringify(own));

      if (size === "phone") { await openDrawer(page); await openDrawer(viewer); }
      const [ourBox, theirBox] = await Promise.all([layout(page), layout(viewer)]);
      const diff = differences(ourBox, theirBox);
      K.layout = { ours: ourBox, differences: diff };
      r.expect(diff.length === 0, key + ": the sidebar's parts sit or read differently from the viewer's: " + diff.join("; "));
      await page.screenshot({ path: path.join(OUT, "embed-" + key + ".png") });
      await viewer.screenshot({ path: path.join(OUT, "viewer-" + key + ".png") });

      if (!dark) {
        // A live poll redraws the list and leaves the page alone.
        const polled = page.waitForResponse((res) => /\/api\/model\?since=/.test(res.url()), { timeout: 8000 }).then(() => true, () => false);
        K.polled = await polled;
        await page.waitForTimeout(150);
        const after = await page.evaluate(() => ({ path: location.pathname, content: !!document.querySelector("#page > #own-content"), rows: document.querySelectorAll("#lanes .srow").length }));
        r.expect(K.polled && after.path === "/__shell-sidebar.html" && after.content && after.rows === ours.length, key + ": a live poll changed the page or lost the list: " + JSON.stringify({ polled: K.polled, ...after }));

        // The search narrows the list as the viewer's does.
        const query = theirs[theirs.length - 1].text.slice(0, 4);
        await page.fill("#q", query); await viewer.fill("#q", query);
        const [ourFound, theirFound] = await Promise.all([lanes(page), lanes(viewer)]);
        K.search = { query, rows: ourFound.length };
        r.expect(ourFound.length > 0 && JSON.stringify(ourFound.map((x) => x.id)) === JSON.stringify(theirFound.map((x) => x.id)), key + ": the search narrows the list differently: " + JSON.stringify({ query, ours: ourFound.map((x) => x.id), theirs: theirFound.map((x) => x.id) }));
        await page.fill("#q", "");

        // A row opens its session in the viewer.
        const row = page.locator("#lanes .srow").first(), id = await row.getAttribute("data-id"), name = await row.locator(".nm").textContent();
        if (size === "phone") await row.tap(); else await row.click();
        const opened = await page.waitForURL((u) => u.pathname.startsWith("/s/"), { timeout: 8000 }).then(() => true, () => false);
        const at = await page.evaluate(() => location.pathname);
        const shown = opened && await page.waitForFunction((n) => document.querySelector("#topbar .t")?.textContent === n, name, { timeout: 8000 }).then(() => true, () => false);
        K.opened = { id, name, at, shown };
        r.expect(opened && decodeURIComponent(at.split("/").pop()) === id && shown, key + ": the row did not open its session's page: " + JSON.stringify(K.opened));
      }

      r.expect(page.errors.length === 0 && viewer.errors.length === 0, key + ": page errors: " + JSON.stringify({ embed: page.errors, viewer: viewer.errors }));
      await page.context().close();
      await viewer.context().close();
    }
  }
  return r.done();
}

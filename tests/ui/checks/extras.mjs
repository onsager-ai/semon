// What only the served viewer does, on the extras fixture (fixture.mjs --extras), phone light unless noted:
//   - paging: the `backlog` lane (471 entries, one unreadable line) opens on its last page with "Load earlier" and no "Started" divider; each
//     "Load earlier" adds entries above without moving what was on screen; the first page ends with the "Started"
//     divider and every one of its 10 turns, with one gap divider where the log lost a line. A deep link (/s/claude/backlog?turn=<an older turn>) lands on that turn,
//     in view, with "Load earlier" above and "Load later" below, and "Load later" reaches the last turn.
//   - a Codex call with no exit status (deps) draws as neither failed nor succeeded: no failed styling, "exit unknown ·",
//     and its group summary counts no failure.
//   - a command longer than its summary (harbor) shows its "Command" section with the whole command.
//   - View all whose fetch fails shows the preview with the "Couldn't load the full text" note; when the fetch works,
//     no note, and the sheet's text is longer than the preview's.
//   - injection: on every screen reached (Home, Analytics, Sessions, Machines, every session including the payload lane,
//     its subagent and the failed send's stub, with every step, card and child run opened, and the details menu, and
//     every trace), the document holds exactly one script (/viewer.js), no img (but the viewer's own harness marks and attachment
//     thumbnails, on /api/attachment) and no iframe,
//     nothing set window.__xss, and the payload shows as text. The payload lane and the failed-send stub also load from
//     their real URLs.
//   - output Codex cut before the model saw it (codex-cut): a plain call's step shows a divider with the count where Codex cut, the note
//     "Codex cut this output before the model saw it" and none of the warning header; a code-mode command cut by the collection cap
//     shows the same in View all. Neither says "Cut short in this copy of the logs". Screenshots at 390 and 1280, light and dark.
//   - spawn cards: the kind badge and the title share one row (phone and desktop, light and dark, also with a long title, and never sideways),
//     the title does not repeat the kind its badge shows, and the Subagent badge in the top bar carries the delegation icon, not the person icon, and the one on a card carries the harness mark and its word, with neither glyph.
//   - no page errors.
import path from "node:path";
import { ENV, served, goto, data, reporter, overflow } from "../lib.mjs";
import { XSS, XSS_KEY } from "../fixture.mjs";

const inView = (page, sel) => page.evaluate((sel) => { const e = document.querySelector(sel), bar = document.querySelector("#topbar").getBoundingClientRect(); if (!e) return null; const r = e.getBoundingClientRect(); return r.top >= bar.bottom - 1 && r.top < innerHeight - 40; }, sel);
const pager = (page) => page.evaluate(() => [...document.querySelectorAll(".turns > .list > button.more")].map((b) => ({ text: b.textContent, first: b.parentElement === document.querySelector(".turns").firstElementChild, last: b.parentElement === document.querySelector(".turns").lastElementChild })));
const pagingAnchor = (page) => page.evaluate(() => {
  const top = innerWidth <= 760 ? 0 : document.querySelector("#main").getBoundingClientRect().top;
  const turn = [...document.querySelectorAll(".turns > .turn[data-turn]")].find((t) => t.getBoundingClientRect().top >= top);
  return turn ? { id: turn.dataset.turn, top: turn.getBoundingClientRect().top } : null;
});
const entryCount = (page) => page.locator(".turns [data-e]").count();
const scrollTranscript = (page, end = false) => page.evaluate((end) => {
  const box = innerWidth <= 760 ? document.scrollingElement : document.querySelector("#main");
  box.scrollTop = end ? box.scrollHeight : 0;
}, end);

async function automaticPaging(browser, size, D, r) {
  const page = await served(browser, { extras: true, size });
  const requests = [], held = [];
  let next = "hold", holdStarted;
  const waitForHold = () => new Promise((resolve) => { holdStarted = resolve; });
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.pathname === "/api/tx" && url.searchParams.get("sid") === "backlog" && url.searchParams.has("before")) requests.push(url.searchParams.get("before"));
  });
  await page.route("**/api/tx?**", async (route) => {
    const url = new URL(route.request().url());
    if (url.searchParams.get("sid") !== "backlog" || !url.searchParams.has("before")) return route.fallback();
    const action = next;
    if (action === "fail") { next = null; return route.fulfill({ status: 500, contentType: "text/plain", body: "paging unavailable" }); }
    if (action === "hold") await new Promise((resolve) => { held.push(resolve); holdStarted(); });
    return route.fallback();
  });
  const opened = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return url.pathname === "/api/tx" && url.searchParams.get("sid") === "backlog" && !url.searchParams.has("before") && !url.searchParams.has("after");
  });
  await page.goto(ENV.extraBase + "/s/claude/backlog", { waitUntil: "load" });
  const range = await (await opened).json();
  await page.waitForSelector(".turns > .turn");
  r.expect(range.from > 0, size + ": backlog opens on a partial last page");
  await page.waitForTimeout(500);
  r.expect(requests.length === 0, size + ": the opening end pin makes no earlier request");
  const before = await entryCount(page);
  const firstHeld = waitForHold();
  await page.mouse.move(size === "phone" ? 350 : 1200, 300);
  await page.mouse.wheel(0, -1); // reader input releases the opening pin
  await scrollTranscript(page);
  await page.waitForFunction(() => document.querySelector("[data-load-earlier]")?.disabled);
  await firstHeld;
  const loading = await page.locator("[data-load-earlier]").evaluate((b) => ({ text: b.textContent, spin: !!b.querySelector(".spin"), disabled: b.disabled }));
  r.expect(loading.text === "Loading earlier…" && loading.spin && loading.disabled, size + ": automatic paging shows its disabled loading control");
  // Scroll again while the response is held: anchoring must use this position, rather than the request's starting point.
  await page.evaluate(() => { const box = innerWidth <= 760 ? document.scrollingElement : document.querySelector("#main"); box.scrollTop += 120; });
  const anchor = await pagingAnchor(page);
  r.expect(anchor !== null, size + ": a turn is available for anchoring during the request");
  await page.locator("[data-load-earlier]").evaluate((b) => { b.click(); b.click(); });
  await page.waitForTimeout(100);
  r.expect(requests.length === 1 && requests[0] === String(range.from), size + ": observer and button do not duplicate the initial before request: " + JSON.stringify(requests));
  // The next page, whether reached immediately within the margin or by another scroll, fails once.
  next = "fail";
  held.shift()();
  await page.waitForFunction((n) => document.querySelectorAll(".turns [data-e]").length > n, before);
  const after = anchor && await page.evaluate((id) => document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]')?.getBoundingClientRect().top ?? null, anchor.id);
  r.expect(after !== null && anchor !== null && Math.abs(after - anchor.top) <= 2, size + ": automatic earlier paging keeps the turn within 2px: " + JSON.stringify({ anchor, after }));
  await scrollTranscript(page);
  await page.waitForFunction(() => document.querySelector("[data-load-earlier]")?.textContent.includes("Retry"));
  const retry = page.locator("[data-load-earlier]");
  r.expect((await retry.textContent()).replace(/\u2009/g, "").replace(/\s+/g, " ") === "Couldn't load earlier entries · Retry", size + ": a failed page offers Retry");
  const failedRequests = requests.slice();
  r.expect(new Set(failedRequests).size === failedRequests.length, size + ": each automatic before boundary is requested only once: " + JSON.stringify(failedRequests));
  await page.waitForTimeout(3000);
  r.expect(requests.length === failedRequests.length, size + ": a failed pager makes no automatic retry for 3 seconds");
  const beforeRetry = await entryCount(page);
  await retry.click();
  await page.waitForFunction((n) => document.querySelectorAll(".turns [data-e]").length > n, beforeRetry);
  r.expect(requests.filter((from) => from === failedRequests.at(-1)).length === 2, size + ": clicking Retry requests the failed boundary once more");

  // A fresh last page still works from the keyboard, even while its opening pin is active.
  await page.reload({ waitUntil: "load" });
  await page.waitForSelector("[data-load-earlier]");
  const keyboardBefore = await entryCount(page), keyboardRequests = requests.length;
  next = "hold";
  const keyboardHeld = waitForHold();
  await page.locator("[data-load-earlier]").evaluate((b) => b.focus({ preventScroll: true }));
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => document.querySelector("[data-load-earlier]")?.disabled);
  await keyboardHeld;
  await page.waitForTimeout(100);
  r.expect(requests.length === keyboardRequests + 1, size + ": Enter starts exactly one earlier request");
  next = null; held.shift()();
  await page.waitForFunction((n) => document.querySelectorAll(".turns [data-e]").length > n, keyboardBefore);

  // The other margin loads toward the end of a middle page without a click.
  const older = D.turns.filter((t) => t.sid === "backlog")[1];
  await page.goto(ENV.extraBase + "/s/claude/backlog?turn=" + encodeURIComponent(older.id), { waitUntil: "load" });
  await page.waitForSelector(".turns > .turn");
  await page.waitForTimeout(100); // the deep link places its turn again two frames later
  await scrollTranscript(page, true);
  await page.waitForFunction(() => document.querySelector(".turns")?.textContent.includes("Backlog triaged: 460 issues read."));
  r.expect(!await page.locator('[data-pager-where="after"]').count(), size + ": scrolling to the end automatically loads the later page");
  r.expect(page.errors.length === 0, size + ": automatic paging page errors: " + page.errors.join(" | "));
  const result = { range: { from: range.from, to: range.to }, loading, anchor, after, failedRequests, requests };
  await page.context().close();
  return result;
}

export default async function (browser) {
  const r = reporter("extras");
  const R = r.results;
  const D = await data({ extras: true });

  // ---- Embedding server account menu and Machines destination -----------------------------------------------------
  {
    const name = '<img src=x onerror="window.__accountXss=1">';
    r.expect(!!ENV.accountBase && !!ENV.accountToken, "the account fixture server is required");
    const desktop = await served(browser, { account: true, size: "desktop" });
    const topbarAvatar = await desktop.locator("#topbar .account-avatar-button").evaluate((button) => {
      const rect = button.getBoundingClientRect();
      return { width: rect.width, height: rect.height, right: Math.round(document.querySelector("#topbar").getBoundingClientRect().right - rect.right), visible: getComputedStyle(button).display !== "none" };
    });
    const servedAccount = await desktop.evaluate(async () => (await (await fetch("/api/model")).json()).account);
    const servedNav = await desktop.evaluate(async () => (await (await fetch("/api/model")).json()).nav);
    r.expect(servedAccount?.name === name && servedAccount.links?.map((link) => link.method).join(",") === "get,get,post" && servedNav?.machines === "/account/workspaces", "the embedding fixture serves account methods and nav model values");
    r.expect(topbarAvatar.width === 32 && topbarAvatar.height === 32 && topbarAvatar.visible && topbarAvatar.right === 20, "desktop has a 32px avatar button at the top bar's right end: " + JSON.stringify(topbarAvatar));
    await desktop.locator("#topbar .account-avatar-button").click();
    const desktopMenu = await desktop.locator("#topbar .account-popover").evaluate((menu) => ({
      name: menu.querySelector(".account-name")?.textContent,
      login: menu.querySelector(".account-login-value")?.textContent,
      workspaces: [...menu.querySelectorAll(".account-workspace-form")].map((form) => {
        const row = form.querySelector(".account-menu-row");
        return { name: row.querySelector(".account-workspace-name")?.textContent, method: form.getAttribute("method"), action: form.getAttribute("action"), tag: row.tagName, type: row.getAttribute("type"), role: row.getAttribute("role"), current: row.getAttribute("aria-current"), controls: form.elements.length, hidden: form.querySelectorAll("input").length };
      }),
      allWorkspaces: [...menu.querySelectorAll(".account-workspace-name")].map((row) => row.textContent),
      links: [...menu.querySelectorAll(".account-links > a, .account-links > form")].map((container) => {
        const form = container.matches("form") ? container : null;
        const row = form ? form.querySelector(".account-menu-row") : container;
        return { label: row.textContent, tag: row.tagName, href: row.getAttribute("href"), method: form?.getAttribute("method") ?? null, action: form?.getAttribute("action") ?? null, danger: row.classList.contains("danger"), role: row.getAttribute("role"), type: row.getAttribute("type"), controls: form?.elements.length ?? null };
      }),
      expanded: document.querySelector("#topbar .account-avatar-button")?.getAttribute("aria-expanded"),
      payloadNodes: { img: menu.querySelectorAll("img").length, script: menu.querySelectorAll("script").length },
    }));
    R.accountDesktop = { topbarAvatar, desktopMenu };
    r.expect(desktopMenu.name === name && desktopMenu.login === "reader@example.invalid", "the menu shows identity strings as text: " + JSON.stringify(desktopMenu));
    r.expect(desktopMenu.allWorkspaces.join(",") === "Research,Writing" && desktopMenu.workspaces.length === 2 && desktopMenu.workspaces.every((row) => row.method === "post" && row.tag === "BUTTON" && row.type === "submit" && row.role === "menuitem" && row.controls === 1 && row.hidden === 0) && desktopMenu.workspaces[0].name === "Research" && desktopMenu.workspaces[0].action === "/workspaces/research" && desktopMenu.workspaces[0].current === "page" && desktopMenu.workspaces[1].action === "/workspaces/writing", "workspace rows are POST forms and mark the current one: " + JSON.stringify(desktopMenu.workspaces));
    r.expect(desktopMenu.links.length === 3 && desktopMenu.links[0].label === "Profile" && desktopMenu.links[0].tag === "A" && desktopMenu.links[0].href === "/account/profile" && desktopMenu.links[1].label === "Machines" && desktopMenu.links[1].tag === "A" && desktopMenu.links[1].href === "/machines" && desktopMenu.links[2].label === "Sign out" && desktopMenu.links[2].tag === "BUTTON" && desktopMenu.links[2].method === "post" && desktopMenu.links[2].action === "/account/sign-out" && desktopMenu.links[2].controls === 1 && desktopMenu.links[2].role === "menuitem" && desktopMenu.links[2].type === "submit" && desktopMenu.links[2].danger, "GET account links remain anchors and Sign out is a POST form: " + JSON.stringify(desktopMenu.links));
    r.expect(desktopMenu.payloadNodes.img === 0 && desktopMenu.payloadNodes.script === 0 && (await desktop.evaluate(() => window.__accountXss ?? null)) === null, "the injected name stays a text node");
    await desktop.keyboard.press("Escape");
    r.expect(await desktop.locator("#topbar .account-popover").count() === 0, "Escape closes the account menu");
    await desktop.locator("#topbar .account-avatar-button").click();
    await desktop.locator("#page").click({ position: { x: 8, y: 8 } });
    r.expect(await desktop.locator("#topbar .account-popover").count() === 0, "a click outside closes the account menu");

    const workspacePost = desktop.waitForResponse((response) => new URL(response.url()).pathname === "/workspaces/research" && response.request().method() === "POST");
    await desktop.locator("#topbar .account-avatar-button").click();
    await desktop.locator("#topbar .account-workspace-form button").first().click();
    const workspaceResponse = await workspacePost;
    const landedAtRoot = await desktop.waitForURL((url) => url.pathname === "/" && url.search === "").then(() => true, () => false);
    const responseBody = await workspaceResponse.text().catch(() => "");
    let viewerLoaded = await desktop.waitForFunction(() => !!document.querySelector('.nav-item[data-go="machines"]')).then(() => true, () => false);
    const landing = { status: workspaceResponse.status(), location: workspaceResponse.headers()["location"] ?? null, body: responseBody, url: desktop.url(), landedAtRoot, viewerLoaded };
    r.expect(landing.status === 303 && landedAtRoot && viewerLoaded, "submitting the workspace form follows its 303 back to /: " + JSON.stringify(landing));
    if (!viewerLoaded) {
      await desktop.goto(ENV.accountBase + "/?t=" + ENV.accountToken, { waitUntil: "load" });
      viewerLoaded = await desktop.waitForFunction(() => !!document.querySelector('.nav-item[data-go="machines"]')).then(() => true, () => false);
    }

    await desktop.route("**/account/workspaces", (route) => route.fulfill({ status: 200, contentType: "text/plain", body: "workspaces destination" }));
    await desktop.locator('.nav-item[data-go="machines"]').click();
    await desktop.waitForURL((url) => url.pathname === "/account/workspaces");
    r.expect(new URL(desktop.url()).pathname === "/account/workspaces", "the Machines nav entry follows the same-origin override");
    await desktop.goto(ENV.accountBase + "/machines?t=" + ENV.accountToken, { waitUntil: "load" });
    await desktop.waitForURL((url) => url.pathname === "/account/workspaces");
    r.expect(new URL(desktop.url()).pathname === "/account/workspaces", "the /machines route follows the override when opened directly");
    await desktop.goto(ENV.accountBase + "/machines/laptop?t=" + ENV.accountToken, { waitUntil: "load" });
    await desktop.waitForFunction(() => document.querySelector("#topbar .crumb")?.textContent === "Machines");
    await desktop.locator("#topbar .crumb").click();
    await desktop.waitForURL((url) => url.pathname === "/account/workspaces");
    r.expect(new URL(desktop.url()).pathname === "/account/workspaces", "the Machines detail crumb follows the override");
    r.expect(desktop.errors.length === 0, "desktop account page errors: " + desktop.errors.join(" | "));
    await desktop.context().close();

    const phone = await served(browser, { account: true, size: "phone" });
    await phone.locator("#lead-btn").click();
    const footer = phone.locator("#account-drawer");
    await footer.scrollIntoViewIfNeeded();
    const drawerRow = await footer.evaluate((widget) => ({
      isLast: widget === widget.parentElement.lastElementChild,
      open: document.body.classList.contains("drawer-open"),
      summary: widget.querySelector(".account-trigger")?.innerText,
      visible: getComputedStyle(widget).display !== "none",
    }));
    r.expect(drawerRow.isLast && drawerRow.open && drawerRow.visible && drawerRow.summary.includes(name) && drawerRow.summary.includes("Research"), "phone account summary is the drawer footer row: " + JSON.stringify(drawerRow));
    await footer.locator(".account-trigger").click();
    const phoneMenu = await footer.locator(".account-popover").evaluate((menu) => ({
      workspaces: [...menu.querySelectorAll(".account-workspace-name")].map((row) => row.textContent),
      current: menu.querySelector('.account-menu-row[aria-current="page"] .account-workspace-name')?.textContent,
      targets: [...menu.querySelectorAll(".account-menu-row")].map((row) => Math.round(row.getBoundingClientRect().height)),
      postForms: [...menu.querySelectorAll("form")].map((form) => ({ method: form.getAttribute("method"), action: form.getAttribute("action"), controls: form.elements.length })),
      visible: getComputedStyle(menu).display !== "none",
    }));
    R.accountPhone = { drawerRow, phoneMenu };
    r.expect(phoneMenu.visible && phoneMenu.workspaces.join(",") === "Research,Writing" && phoneMenu.current === "Research" && phoneMenu.postForms.length === 3 && phoneMenu.postForms.every((form) => form.method === "post" && form.controls === 1) && phoneMenu.postForms[2].action === "/account/sign-out", "phone footer expands to the same account menu with POST forms: " + JSON.stringify(phoneMenu));
    r.expect(phoneMenu.targets.length === 5 && phoneMenu.targets.every((height) => height >= 44), "phone account links have 44px targets: " + JSON.stringify(phoneMenu.targets));
    r.expect(phone.errors.length === 0, "phone account page errors: " + phone.errors.join(" | "));
    await phone.context().close();
  }

  // ---- Paging -------------------------------------------------------------------------------------------------------
  R.automaticPaging = {};
  for (const size of ["phone", "desktop"]) R.automaticPaging[size] = await automaticPaging(browser, size, D, r);
  {
    const page = await served(browser, { extras: true, path: "/s/claude/backlog" });
    // Keep the original fallback-control assertions deterministic in a browser without IntersectionObserver.
    await page.addInitScript(() => { delete window.IntersectionObserver; });
    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(() => !!document.querySelector(".turns"));
    const turns = D.turns.filter((t) => t.sid === "backlog");
    const count = () => page.evaluate(() => ({ turns: document.querySelectorAll(".turns > .turn").length, started: [...document.querySelectorAll(".turns > .divider")].some((d) => d.textContent.startsWith("Started")) }));
    const P = { open: { ...(await count()), pager: await pager(page) }, clicks: [] };
    r.expect(turns.length === 10, "backlog has 10 turns in the model, not " + turns.length);
    r.expect(P.open.pager.length === 1 && P.open.pager[0].text === "Load earlier" && P.open.pager[0].first, "the last page opens with Load earlier at the top: " + JSON.stringify(P.open.pager));
    r.expect(!P.open.started, "no Started divider on a page that doesn't start the transcript");
    r.expect(P.open.turns > 0 && P.open.turns < 10, "the last page holds some of the turns: " + P.open.turns);
    for (let k = 0; k < 5 && (await pager(page)).some((b) => b.text === "Load earlier"); k++) {
      // Bring the button into view first (the click would scroll to it), then note the first turn below the scroller edge.
      await page.locator(".turns > .list > button.more").scrollIntoViewIfNeeded(); await page.waitForTimeout(80);
      const anchor = await pagingAnchor(page);
      const before = await count();
      await page.click(".turns > .list > button.more"); await page.waitForFunction((n) => document.querySelectorAll(".turns > .turn").length > n || ![...document.querySelectorAll(".turns > .list > button.more")].some((b) => b.textContent === "Load earlier"), before.turns);
      await page.waitForTimeout(100);
      const after = await page.evaluate((id) => document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]')?.getBoundingClientRect().top ?? null, anchor.id);
      P.clicks.push({ before: before.turns, after: (await count()).turns, anchorMoved: after == null ? null : Math.round(after - anchor.top) });
    }
    P.done = { ...(await count()), pager: await pager(page) };
    r.expect(P.clicks.length > 0 && P.clicks.every((c) => c.after >= c.before), "Load earlier adds turns: " + JSON.stringify(P.clicks));
    r.expect(P.clicks.every((c) => c.anchorMoved !== null && Math.abs(c.anchorMoved) <= 2), "what was on screen stays put after Load earlier: " + JSON.stringify(P.clicks));
    r.expect(P.done.turns === 10 && P.done.started && P.done.pager.length === 0, "the first page starts with the Started divider and holds all 10 turns: " + JSON.stringify(P.done));
    P.gaps = await page.evaluate(() => [...document.querySelectorAll(".turns > .divider")].filter((d) => d.textContent.startsWith("Some entries not included")).length);
    r.expect(P.gaps === 1, "the unreadable line shows as one gap divider between turns: " + P.gaps);
    await page.screenshot({ path: path.join(ENV.out, "extras-backlog-start.png") });
    // A deep link to an older turn.
    const older = turns[1];
    await page.goto(ENV.extraBase + "/s/claude/backlog?turn=" + encodeURIComponent(older.id), { waitUntil: "load" });
    await page.waitForFunction((id) => !!document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]'), older.id);
    await page.waitForTimeout(300);
    P.deep = { inView: await inView(page, '.turn[data-turn="' + older.id.replace(/"/g, '\\"') + '"]'), pager: await pager(page), url: await page.evaluate(() => location.pathname + location.search) };
    r.expect(P.deep.inView === true, "the deep-linked turn lands in view");
    r.expect(P.deep.pager.some((b) => b.text === "Load earlier" && b.first) && P.deep.pager.some((b) => b.text === "Load later" && b.last), "a middle page has Load earlier above and Load later below: " + JSON.stringify(P.deep.pager));
    r.expect(P.deep.url === "/s/claude/backlog?turn=" + encodeURIComponent(older.id), "the URL keeps the turn: " + P.deep.url);
    for (let k = 0; k < 5 && (await pager(page)).some((b) => b.text === "Load later"); k++) {
      await page.click(".turns > .list:last-child > button.more"); await page.waitForTimeout(250);
    }
    P.later = await page.evaluate((id) => ({ last: !!document.querySelector('.turn[data-turn="' + CSS.escape(id) + '"]'), text: document.querySelector(".turns").textContent.includes("Backlog triaged: 460 issues read."), pager: [...document.querySelectorAll(".turns > .list > button.more")].map((b) => b.textContent) }), turns.at(-1).id);
    r.expect(P.later.last && P.later.text && !P.later.pager.includes("Load later"), "Load later reaches the last turn: " + JSON.stringify(P.later));
    R.paging = P;
    r.expect(page.errors.length === 0, "paging: page errors " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- Steps: no exit status, a long command, View all ------------------------------------------------------------------
  {
    const page = await served(browser, { extras: true, path: "/s/codex/deps" });
    await page.waitForFunction(() => !!document.querySelector(".turns"));
    await page.evaluate(() => document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((b) => b.click()));
    R.unknown = await page.evaluate(() => {
      const norm = (s) => String(s ?? "").replace(/\u2009/g, " ").replace(/\s+/g, " ").trim();
      const step = [...document.querySelectorAll(".step")].find((s) => norm(s.querySelector(".sd")?.textContent).startsWith("exit unknown · "));
      const sum = step?.closest(".tgroup")?.querySelector(".tsum");
      return step ? { err: step.classList.contains("err"), sd: norm(step.querySelector(".sd").textContent), groupFailed: !!sum?.querySelector(".tf"), grouped: !!sum } : null;
    });
    r.expect(R.unknown !== null, "a step with no exit status reads \"exit unknown · …\"");
    r.expect(R.unknown && !R.unknown.err && R.unknown.grouped && !R.unknown.groupFailed, "an unknown exit is neither failed nor counted as failed: " + JSON.stringify(R.unknown));
    R.shortCommand = await page.evaluate(() => {
      const step = [...document.querySelectorAll(".step")].find((item) => item.querySelector(".sa")?.textContent === "cargo metadata --format-version 1 --no-deps");
      if (!step) return null;
      const button = step.querySelector(":scope > button"); if (button?.getAttribute("aria-expanded") === "false") button.click();
      const out = step.querySelector(":scope > .out"), labels = [...out.querySelectorAll(":scope > .io")].map((label) => label.textContent);
      return { command: out.querySelector("pre.in")?.textContent ?? null, labels, cwd: labels.find((label) => label.startsWith("Working directory")) ?? null };
    });
    r.expect(R.shortCommand?.command === "cargo metadata --format-version 1 --no-deps" && R.shortCommand.labels.indexOf("Command") === 0 && R.shortCommand.labels.indexOf("Output") > R.shortCommand.labels.indexOf("Command") && R.shortCommand.cwd === null, "a short shell detail shows Command then Output and hides the session-root directory: " + JSON.stringify(R.shortCommand));

    await page.goto(ENV.extraBase + "/s/claude/harbor", { waitUntil: "load" }); await page.waitForFunction(() => !!document.querySelector(".turns"));
    await page.evaluate(() => document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((b) => b.click()));
    R.command = await page.evaluate(() => {
      const b = [...document.querySelectorAll(".step > button")].find((x) => x.querySelector(".sa")?.textContent.endsWith("…"));
      if (!b) return null; b.click(); const out = b.parentElement.querySelector(".out");
      return { label: out.querySelector(".io")?.textContent ?? null, input: out.querySelector("pre.in")?.textContent ?? "", summary: b.querySelector(".sa").textContent };
    });
    r.expect(R.command !== null, "harbor has a step whose summary is cut");
    r.expect(R.command && R.command.label === "Command" && R.command.input.length > R.command.summary.length && R.command.input.includes("--nocapture"), "the long command shows whole under Command: " + JSON.stringify(R.command && { label: R.command.label, input: R.command.input.length, summary: R.command.summary.length }));

    // View all on the cut output: first with /api/entry failing, then working.
    const openAll = async () => {
      // The long command's step: its output is longer than the server's preview.
      await page.evaluate(() => { const b = [...document.querySelectorAll(".step > button")].find((x) => x.querySelector(".sa")?.textContent.endsWith("…")); b?.parentElement.setAttribute("data-long", ""); if (b && b.getAttribute("aria-expanded") === "false") b.click(); });
      await page.waitForTimeout(100);
      const btn = page.locator(".step[data-long] .viewall:visible").first();
      r.expect(await btn.count() === 1, "harbor's server-cut output offers View all");
      await btn.click(); await page.waitForSelector("dialog.viewer[open]"); await page.waitForTimeout(200);
      return page.evaluate(() => { const d = document.querySelector("dialog.viewer"); return { notes: [...d.querySelectorAll(".vnote")].map((n) => n.textContent), out: [...d.querySelectorAll("pre")].at(-1)?.textContent.length ?? 0 }; });
    };
    const preview = await page.evaluate(() => { const b = [...document.querySelectorAll(".step > button")].find((x) => x.querySelector(".sa")?.textContent.endsWith("…")); if (b?.getAttribute("aria-expanded") === "false") b.click(); return [...(b?.parentElement.querySelectorAll(".out pre.clip") ?? [])].at(-1)?.textContent.length ?? 0; });
    r.expect(preview > 0 && preview <= 1536 + 3, "the preview is the server's cut: " + preview);
    await page.route("**/api/entry**", (x) => x.abort());
    const failed = await openAll(); await page.click(".viewer .vclose"); await page.waitForTimeout(250);
    await page.unroute("**/api/entry**");
    const ok = await openAll(); await page.click(".viewer .vclose"); await page.waitForTimeout(250);
    R.viewAll = { preview, failed, ok };
    r.expect(failed.notes.length === 1 && failed.notes[0].startsWith("Couldn't load the full text") && failed.out === preview, "a failed fetch shows the preview with only its note: " + JSON.stringify(failed));
    r.expect(ok.notes.length === 0 && ok.out > preview, "a working fetch shows the whole text, longer than the preview: " + JSON.stringify(ok));
    r.expect(page.errors.length === 0, "steps: page errors " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- Code-mode exec: unwrapped operations and its script control -----------------------------------------------
  {
    const page = await served(browser, { extras: true, path: "/s/codex/code-mode" });
    await page.waitForFunction(() => !!document.querySelector(".turns"));
    const data = await page.evaluate(async () => {
      const token = new URLSearchParams(location.search).get("t");
      const response = await fetch("/api/tx?sid=code-mode&t=" + encodeURIComponent(token));
      const totals = await response.json();
      document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((button) => button.click());
      const step = [...document.querySelectorAll(".step")].find((item) => item.querySelector(".sa")?.textContent === "git status");
      const toggle = step?.querySelector(":scope > button"); if (toggle?.getAttribute("aria-expanded") === "false") toggle.click();
      const detail = step?.querySelector(":scope > .out");
      return {
        calls: totals.calls,
        errors: totals.errors,
        steps: [...document.querySelectorAll(".step")].map((step) => ({
          arg: step.querySelector(".sa")?.textContent ?? "",
          err: step.classList.contains("err"),
        })),
        groupedSteps: document.querySelectorAll(".tgroup .steps > .step").length,
        groupScriptButtons: document.querySelectorAll(".tgroup > .viewscript").length,
        detailScriptButtons: [...(detail?.querySelectorAll(".viewscript") ?? [])].map((button) => button.textContent),
        detailLabels: [...(detail?.querySelectorAll(":scope > .io") ?? [])].map((label) => label.textContent),
      };
    });
    R.codeMode = data;
    r.expect(data.calls === 3 && data.errors === 0, "three indexed operations are counted: " + JSON.stringify({ calls: data.calls, errors: data.errors }));
    r.expect(data.steps.map((step) => step.arg).join("|") === "git status|sed -n '1,9p' a.rs|src/code-mode.rs", "the steps show unwrapped commands and the changed path: " + JSON.stringify(data.steps));
    r.expect(data.steps.length === 3 && data.steps.every((step) => !step.err), "three ordinary, successful tool steps are shown");
    r.expect(data.groupedSteps === 3, "the script-backed operation remains inside a multi-step group: " + data.groupedSteps);
    r.expect(data.groupScriptButtons === 0, "script controls never sit orphaned on the group summary");
    r.expect(data.detailScriptButtons.length === 1 && data.detailScriptButtons[0] === "View script", "an expanded code-mode step has exactly one View script action: " + JSON.stringify(data.detailScriptButtons));
    r.expect(data.detailLabels.indexOf("Command") === 0 && data.detailLabels.indexOf("Output") > data.detailLabels.indexOf("Command") && !data.detailLabels.includes("Working directory · ."), "a short code-mode operation shows Command and Output without the session-root directory: " + JSON.stringify(data.detailLabels));
    await page.locator(".step > .out:not([hidden]) .viewscript").click(); await page.waitForSelector("dialog.viewer[open]");
    R.codeMode.script = await page.locator(".viewer pre.script").textContent();
    r.expect(R.codeMode.script.includes("Promise.allSettled") && R.codeMode.script.includes("git status"), "View script opens the source in the existing sheet");
    await page.click(".viewer .vclose");
    r.expect(page.errors.length === 0, "code-mode page errors: " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- Yielded Codex command and poll input ---------------------------------------------------------------------
  {
    const page = await served(browser, { extras: true, path: "/s/codex/yielded-ui" });
    await page.waitForFunction(() => !!document.querySelector(".turns"));
    const data = await page.evaluate(async () => {
      const token = new URLSearchParams(location.search).get("t");
      const response = await fetch("/api/tx?sid=yielded-ui&t=" + encodeURIComponent(token));
      const tx = await response.json(), entries = tx.entries.filter((entry) => entry.k === "tool");
      const commandEntry = entries.find((entry) => entry.name === "exec_command"), inputEntry = entries.find((entry) => entry.name === "write_stdin");
      document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((button) => button.click());
      const steps = [...document.querySelectorAll(".step")];
      const commandStep = steps.find((step) => step.querySelector(".sv")?.textContent === "Ran");
      const inputStep = steps.find((step) => step.querySelector(".sv")?.textContent === "Sent input to");
      for (const step of [commandStep, inputStep]) { const button = step?.querySelector(":scope > button"); if (button?.getAttribute("aria-expanded") === "false") button.click(); }
      const detail = (step) => {
        const out = step?.querySelector(":scope > .out");
        return { labels: [...(out?.querySelectorAll(":scope > .io") ?? [])].map((label) => label.textContent), value: out?.querySelector("pre.in")?.textContent ?? null };
      };
      return { commandEntry, inputEntry, command: detail(commandStep), input: detail(inputStep) };
    });
    R.yielded = data;
    r.expect(data.commandEntry?.in?.startsWith("printf ") && data.commandEntry.arg.endsWith("…"), "the yielded command has a full input alongside its short summary: " + JSON.stringify(data.commandEntry));
    r.expect(data.command.value === data.commandEntry.in && data.command.labels[0] === "Command" && data.command.labels.indexOf("Output") > data.command.labels.indexOf("Command"), "the yielded exec_command step shows its full command under Command, then Output: " + JSON.stringify(data.command));
    r.expect(data.inputEntry?.in === "y\n" && data.input.value === "y\n" && data.input.labels[0] === "Input" && data.input.labels.indexOf("Output") > data.input.labels.indexOf("Input"), "the write_stdin step shows its sent text under Input, then Output: " + JSON.stringify(data.input));
    r.expect(page.errors.length === 0, "yielded Codex steps have page errors: " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- Output Codex cut before the model saw it ----------------------------------------------------------------
  {
    R.codexCut = {};
    for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
      const tag = size + (dark ? "-dark" : "-light");
      const page = await served(browser, { extras: true, size, dark, path: "/s/codex/codex-cut" });
      await page.waitForFunction(() => !!document.querySelector(".turns"));
      await page.evaluate(() => { for (let i = 0; i < 3; i++) document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((b) => b.click()); document.querySelectorAll('.step > button[aria-expanded="false"]').forEach((b) => b.click()); });
      const steps = await page.evaluate(() => [...document.querySelectorAll(".step")].map((s) => ({
        arg: s.querySelector(".sa")?.textContent,
        gaps: [...s.querySelectorAll(".out .cutgap")].map((g) => g.textContent),
        note: s.querySelector(".out .cutnote")?.textContent ?? null,
        texts: [...s.querySelectorAll(".out .cutout pre")].map((p) => p.textContent),
        viewAll: !!s.querySelector(".viewall:not([hidden])"),
      })));
      const plain = steps.find((s) => s.arg?.startsWith("cargo test")), capped = steps.find((s) => s.arg?.startsWith("cat build.log"));
      r.expect(plain && capped, tag + ": both codex-cut steps are shown: " + JSON.stringify(steps.map((s) => s.arg)));
      if (!plain || !capped) { await page.context().close(); continue; }
      r.expect(plain.gaps.length === 1 && plain.gaps[0] === "About 19,500 tokens cut here by Codex", tag + ": the plain call shows its token gap with its count: " + JSON.stringify(plain.gaps));
      r.expect(plain.texts.length === 2 && plain.texts[0].startsWith("test suite::case_000") && plain.texts[1].includes("test result: ok. 900 passed"), tag + ": the head and the tail are on either side of the gap: " + JSON.stringify(plain.texts));
      r.expect(plain.note?.startsWith("Codex cut this output before the model saw it") && plain.note.includes("24,000 tokens in all"), tag + ": the note says Codex cut it: " + plain.note);
      r.expect(!plain.texts.some((t) => t.includes("Warning: truncated output") || t.includes("Total output lines")), tag + ": the warning header is not shown as output");
      r.expect(!plain.viewAll, tag + ": a short cut output has no View all");
      r.expect(capped.viewAll, tag + ": the capped command offers View all");
      r.expect(!(await page.evaluate(() => document.body.textContent.includes("Cut short in this copy of the logs"))), tag + ": no step says the logs were cut");
      r.expect((await overflow(page)) === 0, tag + ": nothing overflows with the steps open");
      const plainStep = page.locator(".step", { has: page.locator('.sa:text-matches("^cargo test")') });
      await plainStep.scrollIntoViewIfNeeded(); await page.waitForTimeout(150);
      await page.screenshot({ path: path.join(ENV.out, "codex-cut-step-" + tag + ".png") });
      await page.locator(".step", { has: page.locator('.sa:text-matches("^cat build.log")') }).locator(".viewall").click();
      await page.waitForSelector("dialog.viewer[open] .cutgap"); await page.waitForTimeout(200);
      const sheet = await page.evaluate(() => { const v = document.querySelector("dialog.viewer[open]"); return { gaps: [...v.querySelectorAll(".cutgap")].map((g) => g.textContent), notes: [...v.querySelectorAll(".vnote")].map((n) => n.textContent), first: v.querySelector(".cutout pre")?.textContent.split("\n").length ?? 0, last: v.querySelector(".cutout pre:last-of-type")?.textContent.trim().split("\n").pop() ?? null }; });
      r.expect(sheet.gaps.length === 1 && sheet.gaps[0] === "1,048,576 bytes cut here by Codex", tag + ": View all shows the collection gap with its count: " + JSON.stringify(sheet.gaps));
      r.expect(sheet.first === 40 && sheet.last === "[9999] compiled unit 9999", tag + ": View all shows the whole head: " + JSON.stringify(sheet));
      r.expect(sheet.notes.some((n) => n.startsWith("Codex cut this output before the model saw it")) && !sheet.notes.some((n) => n.includes("Cut short in this copy")), tag + ": View all says Codex cut it: " + JSON.stringify(sheet.notes));
      r.expect((await overflow(page)) === 0, tag + ": nothing overflows with View all open");
      await page.locator("dialog.viewer[open] .cutgap").scrollIntoViewIfNeeded(); await page.waitForTimeout(150);
      await page.screenshot({ path: path.join(ENV.out, "codex-cut-sheet-" + tag + ".png") });
      R.codexCut[tag] = { plain, sheet };
      r.expect(page.errors.length === 0, tag + ": codex-cut page errors: " + page.errors.join(" | "));
      await page.context().close();
    }
  }

  // ---- Result handoff: the transcript keeps the reply once and shows a compact marker ----------------------------
  {
    const page = await served(browser, { extras: true, path: "/s/claude/result-card" });
    await page.waitForFunction(() => !!document.querySelector(".result-marker"));
    const result = await page.locator(".turns").evaluate((turns) => {
      const phrase = "Unique result text for the transcript check.";
      const text = turns.innerText;
      const marker = turns.querySelector(".result-marker");
      return {
        phraseCount: text.split(phrase).length - 1,
        markerText: marker?.innerText ?? "",
        markerCount: turns.querySelectorAll(".result-marker").length,
        markerHasCard: !!marker?.closest(".hcard"),
        moreButtons: turns.querySelectorAll(".result-marker .more").length,
      };
    });
    R.resultMarker = result;
    r.expect(result.phraseCount === 1, "the reply text appears once in the transcript: " + JSON.stringify(result));
    r.expect(result.markerCount === 1 && !result.markerHasCard && result.moreButtons === 0, "the result is one compact marker without a card or Show more: " + JSON.stringify(result));
    const resultId = D.H.find((h) => h.from === "result-card" && h.ask === "result")?.id ?? "";
    await page.click(".turn-end .tracebtn");
    await page.waitForSelector('.flow .hop[data-h="' + resultId + '"]');
    const trace = await page.locator(".flow").evaluate((flow, id) => {
      const phrase = "Unique result text for the transcript check.";
      const result = flow.querySelector('.hop[data-h="' + CSS.escape(id) + '"]');
      return {
        phraseCount: flow.innerText.split(phrase).length - 1,
        hopText: result?.querySelector(".sent")?.innerText ?? "",
        briefs: result?.querySelectorAll(".brief").length ?? 0,
        moreButtons: result?.querySelectorAll(".more").length ?? 0,
      };
    }, resultId);
    R.resultTrace = trace;
    r.expect(trace.phraseCount === 0 && trace.hopText.length > 0 && trace.briefs === 0 && trace.moreButtons === 0, "the trace keeps result text out of its compact marker: " + JSON.stringify(trace));
    r.expect(page.errors.length === 0, "result marker page errors: " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- Needs you: opening a result persists its read state; unavailable storage leaves pages usable -----------------
  {
    const resultHandoff = D.H.find((h) => h.kind === "toyou" && h.ask === "result" && h.from === "result-card");
    r.expect(!!resultHandoff, "the synthetic human-started turn has a result handoff");
    const page = await served(browser, { extras: true, path: "/" });
    const selector = '.sec-h + .list .ib[data-h="' + (resultHandoff?.id ?? "") + '"]';
    await page.waitForFunction((id) => [...document.querySelectorAll(".sec-h")].some((head) => head.firstChild?.textContent === "Needs you") && !!document.querySelector('.sec-h + .list .ib[data-h="' + CSS.escape(id) + '"]'), resultHandoff?.id ?? "");
    const before = await page.locator(selector).evaluate((card) => ({
      brief: card.querySelector(".q")?.innerText ?? "",
      resultIds: [...(card.parentElement.querySelectorAll(".ib[data-h]") ?? [])].map((item) => item.dataset.h),
      badge: document.querySelector('.nav-item[data-go="home"] .cnt.hot')?.textContent ?? "",
      needsYou: card.parentElement.innerText,
    }));
    r.expect(before.brief.includes("Unique result text for the transcript check.") && before.needsYou.includes("Unique result text for the transcript check."), "Needs you shows the full result card text: " + JSON.stringify(before));
    r.expect(before.resultIds.includes(resultHandoff?.id), "the unread result is grouped with Needs you: " + JSON.stringify(before));
    const waitingKinds = before.resultIds.map((id) => D.H.find((h) => h.id === id)?.ask);
    r.expect(waitingKinds.every((ask) => ask === "question" || ask === "decision" || ask === "result") && waitingKinds.includes("result"), "Needs you lists open questions, decisions, and results: " + JSON.stringify(waitingKinds));
    r.expect(Number(before.badge) === before.resultIds.length, "the Home badge counts all open inbox items: " + JSON.stringify(before));

    await page.goto(ENV.extraBase + "/s/claude/result-card?t=" + ENV.extraToken, { waitUntil: "load" });
    await page.waitForFunction(() => !!document.querySelector(".result-marker"));
    const stored = await page.evaluate(() => JSON.parse(localStorage.getItem("semon.seen") ?? "[]"));
    const marker = await page.locator(".result-marker").innerText();
    r.expect(stored.includes(resultHandoff?.id), "opening the session stores its result id as seen: " + JSON.stringify(stored));
    r.expect(marker.includes("read"), "the opened session shows the result as read: " + marker);
    await page.goto(ENV.extraBase + "/?t=" + ENV.extraToken, { waitUntil: "load" });
    await page.waitForFunction(() => document.querySelector("#topbar .t")?.textContent === "Home");
    const onHome = () => page.evaluate((id) => { const head = [...document.querySelectorAll(".sec-h")].find((item) => item.firstChild?.textContent === "Needs you"); return head?.nextElementSibling?.querySelector('.ib[data-h="' + CSS.escape(id) + '"]') ? 1 : 0; }, resultHandoff?.id ?? "");
    r.expect(await onHome() === 0, "the read result leaves the Needs-you inbox");
    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(() => document.querySelector("#topbar .t")?.textContent === "Home");
    r.expect(await onHome() === 0, "the read result stays cleared after reloading Home");
    await page.context().close();

    const blocked = await served(browser, { extras: true, path: "/" });
    await blocked.addInitScript(() => {
      Storage.prototype.getItem = function () { throw new Error("storage unavailable"); };
      Storage.prototype.setItem = function () { throw new Error("storage unavailable"); };
    });
    await blocked.reload({ waitUntil: "load" });
    await blocked.waitForFunction(() => document.querySelector("#topbar .t")?.textContent === "Home");
    await blocked.goto(ENV.extraBase + "/s/claude/result-card?t=" + ENV.extraToken, { waitUntil: "load" });
    await blocked.waitForFunction(() => !!document.querySelector(".result-marker"));
    r.expect(blocked.errors.length === 0, "Home and the session page render when localStorage throws: " + blocked.errors.join(" | "));
    await blocked.context().close();
  }

  // ---- Injection -------------------------------------------------------------------------------------------------------
  {
    const X = { screens: 0, bad: [], payloadShown: 0 };
    const scan = async (page, where) => {
      const s = await page.evaluate((text) => ({ scripts: [...document.querySelectorAll("script")].map((x) => x.getAttribute("src")), img: [...document.querySelectorAll("img")].filter((x) => !(x.matches("span.hicon > img") && /^\/harness\/(claude-code|codex|codex-black|opencode-light|opencode-dark)\.svg$/.test(x.getAttribute("src") ?? "") && x.getAttribute("alt") === "" && [null, "hi-light", "hi-dark"].includes(x.getAttribute("class")) && [...x.attributes].every((a) => ["class", "alt", "src", "draggable", "loading", "decoding"].includes(a.name))) && !(x.matches("button.attach > img.attach-img, dialog.image-viewer img.attach-full") && /^\/api\/attachment\?sid=[^&]*&o=\d+&b=\d+&v=[0-9a-f]{16}$/.test(x.getAttribute("src") ?? "") && [...x.attributes].every((a) => ["class", "alt", "src", "width", "height", "loading", "decoding"].includes(a.name)))).length, iframe: document.querySelectorAll("iframe").length, xss: window.__xss ?? null, shown: document.body.textContent.includes(text) }), "<script>window.__xss=2</script>");
      X.screens++; if (s.shown) X.payloadShown++;
      if (s.scripts.length !== 1 || s.scripts[0] !== "/viewer.js" || s.img || s.iframe || s.xss !== null) X.bad.push(where + ": " + JSON.stringify(s));
    };
    const openEverything = (page) => page.evaluate(() => { for (let k = 0; k < 3; k++) document.querySelectorAll('.cw-toggle[aria-expanded="false"], .tsum[aria-expanded="false"], .step > button[aria-expanded="false"]').forEach((x) => x.click()); document.querySelectorAll(".hcard .more:not([hidden]), .hop .more:not([hidden])").forEach((x) => x.click()); });
    for (const [size, dark] of [["phone", false], ["desktop", true]]) {
      const page = await served(browser, { extras: true, size, dark });
      for (const v of ["home", "analytics", "sessions", "machines"]) { await goto(page, { v }, D); await scan(page, size + " " + v); }
      const sids = [...Object.keys(D.SESS), "unsent:" + XSS];
      for (const id of sids) {
        await goto(page, { v: "session", id }, D); await page.waitForTimeout(100); await openEverything(page); await page.waitForTimeout(80);
        await scan(page, size + " session " + id.slice(0, 20));
        const traces = await page.evaluate(() => [...document.querySelectorAll(".turn-end .tracebtn")].map((b) => b.closest(".turn").dataset.turn));
        for (const t of traces) { await goto(page, { v: "trace", sid: id, turn: t }, D); await openEverything(page); await scan(page, size + " trace " + t.slice(0, 20)); }
      }
      // The payload lane's details menu.
      await goto(page, { v: "session", id: XSS_KEY }, D); await openEverything(page);
      await page.click("#more-btn"); await page.waitForTimeout(150); await scan(page, size + " details menu");
      await page.keyboard.press("Escape");
      X[size + "Errors"] = page.errors;
      r.expect(page.errors.length === 0, size + " injection walk: page errors " + page.errors.join(" | "));
      await page.context().close();
    }
    // The payload lane and the failed send's stub from their real URLs.
    for (const [id, harness] of [[XSS_KEY, "claude"], ["unsent:" + XSS, "claude"]]) {
      const page = await served(browser, { extras: true, path: "/s/" + harness + "/" + encodeURIComponent(id) });
      await page.waitForFunction(() => !!document.querySelector(".turns"));
      const title = await page.evaluate(() => document.querySelector("#topbar .t")?.textContent ?? null);
      await scan(page, "url " + id.slice(0, 20));
      r.expect(title === XSS, "the real URL opens " + id.slice(0, 24) + "… with its name as text: " + JSON.stringify(title));
      r.expect(page.errors.length === 0, "url " + id.slice(0, 20) + ": page errors " + page.errors.join(" | "));
      await page.context().close();
    }
    R.injection = X;
    r.expect(X.bad.length === 0, "screens with a script, img or iframe from content, or __xss set: " + X.bad.slice(0, 5).join(" || "));
    r.expect(X.payloadShown > 10, "the payload shows as text on the screens that carry it: " + X.payloadShown + " of " + X.screens);
  }
  // ---- Spawn cards name the kind once, on the title row, and a subagent's icon is not the person icon ---------------
  {
    const kid = Object.values(D.SESS).find((s) => s.kind === "Subagent" && D.H.some((h) => h.kind === "spawn" && h.to === s.id));
    r.expect(!!kid, "the extras fixture needs a subagent with a spawn handoff");
    if (kid) {
      const parent = D.H.find((h) => h.kind === "spawn" && h.to === kid.id).from;
      const cards = R.spawnCards = {};
      for (const size of ["phone", "desktop"]) for (const dark of [false, true]) {
        const tag = size + "-" + (dark ? "dark" : "light");
        const page = await served(browser, { extras: true, size, dark });
        await goto(page, { v: "session", id: parent }, D); await page.waitForTimeout(150);
        const probe = () => page.evaluate(() => {
          const PERSON = "a4 4 0 1 0 0-8";
          return [...document.querySelectorAll(".hcard.child-card")].map((c) => {
            const head = c.querySelector(":scope > .child-head"), badge = head?.querySelector(".child-kind"), title = head?.querySelector(".ln");
            const b = badge?.getBoundingClientRect(), t = title?.getBoundingClientRect(), kind = badge?.textContent.trim() ?? "";
            const svg = badge?.querySelector("svg");
            return { kind, title: title?.textContent ?? "", inHead: !!head, dTop: b && t ? Math.round(Math.abs(b.top - t.top) * 10) / 10 : null, badgeLeftOfTitle: b && t ? b.right <= t.left + 0.5 : false, badgeWraps: b ? b.height > 24 : true, delegate: svg?.classList.contains("kind-delegate") ?? false, mark: !!badge?.querySelector(":scope > .hicon"), person: !!svg && [...svg.querySelectorAll("path")].some((p) => p.getAttribute("d").includes(PERSON)) };
          });
        });
        const before = await probe();
        cards[tag] = before.length;
        r.expect(before.length > 0, tag + ": no spawn cards to check");
        for (const c of before) {
          r.expect(c.inHead && c.dTop <= 3 && c.badgeLeftOfTitle && !c.badgeWraps, tag + ": the kind badge and the title share one row: " + JSON.stringify(c));
          r.expect(!c.title.includes(c.kind) && !/·\s*(Subagent|Codex run|Relayed)\s*$/.test(c.title), tag + ": the title repeats the kind its badge shows: " + JSON.stringify(c.title));
          // The card's badge names the harness by its mark and the kind by its word: no delegation glyph and no person icon (the top bar's chip, below, keeps the glyph).
          r.expect(c.mark, tag + ": the kind badge has no harness mark: " + JSON.stringify(c));
          if (c.kind === "Subagent") r.expect(!c.delegate && !c.person, tag + ": the Subagent badge should carry the harness mark and its word, with no delegation glyph or person icon: " + JSON.stringify(c));
        }
        // A long title wraps beside the badge (the badge keeps its row and its width) and never widens the page.
        await page.evaluate(() => { for (const t of document.querySelectorAll(".hcard.child-card .child-head .ln")) t.textContent = "A deliberately long handoff title that has to wrap onto a second and a third line on a phone " + t.textContent; });
        const after = await probe();
        for (const c of after) r.expect(c.dTop <= 3 && c.badgeLeftOfTitle && !c.badgeWraps, tag + ": with a long title the badge and the title's first line still share a row: " + JSON.stringify(c));
        const sideways = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth);
        r.expect(sideways <= 0, tag + ": the long title pushed the page " + sideways + "px sideways");
        await page.locator(".hcard.child-card").first().scrollIntoViewIfNeeded();
        await page.screenshot({ path: path.join(ENV.out, "spawn-card-" + tag + ".png") });
        if (size === "desktop") {
          await goto(page, { v: "session", id: kid.id }, D);
          const meta = await page.evaluate(() => { const svg = document.querySelector("#topbar .meta-kind svg"); return { present: !!svg, delegate: svg?.classList.contains("kind-delegate") ?? false, person: !!svg && [...svg.querySelectorAll("path")].some((p) => p.getAttribute("d").includes("a4 4 0 1 0 0-8")) }; });
          r.expect(meta.present && meta.delegate && !meta.person, tag + ": the subagent's top bar meta line should carry the delegation icon: " + JSON.stringify(meta));
        }
        r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join(" | "));
        await page.context().close();
      }
    }
  }

  return r.done();
}

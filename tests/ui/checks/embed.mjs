// What an embedding page can do with the viewer: listen for `semon:ended` (and cancel it), ask for a poll with
// `semon:refresh`, watch `semon:polled`, and hand it an account menu in `window.semonEmbed.account`. Runs on the primary
// fixture's viewer at 1280×860, answering the polls itself (a real 304, a full 200, a 500 or a 403) through Playwright routes:
//   - cancelling `semon:ended` keeps the "Session ended" note away when the server answers 403; not cancelling shows it, and the
//     event says which status ended the session;
//   - dispatching `semon:refresh` during a backoff polls within 100 ms, and the poll after it comes 2 s later, not at the
//     backed-off delay;
//   - `semon:polled` says `ok` for a 200 and a 304, and not for a 500;
//   - `window.semonEmbed.account`, set before the page loads, gives the account menu, all text (no markup made from the name);
//     an href that is not a same-origin path (`javascript:`, `//host`, a backslash) rejects the menu, the same as a server's;
//   - a menu the server provides wins over the embedding page's; an invalid one from the server leaves the embedding page's.
// Screenshots of the open menu are written to out/embed/.
import fs from "node:fs";
import path from "node:path";
import { ENV, context, settled, reporter } from "../lib.mjs";

const OUT = path.join(ENV.out, "embed");
fs.mkdirSync(OUT, { recursive: true });

const ENDED = "Session ended: reload with the printed URL";
const account = (over = {}) => ({
  name: "Embed <b>Ada</b> Lovelace", login: "ada", initials: "AL",
  workspaces: [{ name: "Engine room", role: "owner", current: true, switch_href: "/embed/switch/1" }],
  links: [{ label: "Profile", href: "/embed/profile", method: "get", danger: false }, { label: "Sign out", href: "/embed/sign-out", method: "post", danger: true }],
  ...over,
});
const sleep = (n) => new Promise((resolve) => setTimeout(resolve, n));

// A page on the fixture's viewer. `state.mode` picks how a poll is answered: "pass" (the server's own answer, a 304 when
// nothing changed), "full" (a 200 with the whole model), "500" or "403". The boot request, the only one without `since`, goes
// through, with `serverAccount` added to the model when given (null: an invalid account, to test the fallback).
async function open(browser, { embed, state = { mode: "pass" }, serverAccount } = {}) {
  const ctx = await context(browser, { size: "desktop", dark: false });
  await ctx.addInitScript((embedded) => {
    if (embedded) window.semonEmbed = embedded;
    window.__ev = []; window.__fetches = []; window.__cancel = false;
    for (const type of ["semon:ended", "semon:polled"]) {
      window.addEventListener(type, (e) => { if (type === "semon:ended" && window.__cancel) e.preventDefault(); window.__ev.push({ type, detail: e.detail, cancelable: e.cancelable, prevented: e.defaultPrevented }); });
    }
    const orig = window.fetch;
    window.fetch = function (...a) { if (/\/api\/model\?since=/.test(String(a[0]))) window.__fetches.push(performance.now()); return orig.apply(window, a); };
  }, embed ?? null);
  const page = await ctx.newPage();
  page.errors = [];
  page.on("pageerror", (e) => page.errors.push(e.message.split("\n")[0]));
  page.setDefaultTimeout(8000);
  await page.route(/.*/, (r) => (r.request().url().startsWith(ENV.base + "/") ? r.fallback() : r.abort()));
  await page.route("**/api/model**", async (route) => {
    const url = new URL(route.request().url());
    if (!url.searchParams.has("since")) {
      if (serverAccount === undefined) return route.continue();
      const model = await (await route.fetch()).json();
      return route.fulfill({ status: 200, headers: { "content-type": "application/json" }, body: JSON.stringify({ ...model, account: serverAccount }) });
    }
    if (state.mode === "500") return route.fulfill({ status: 500, body: "no" });
    if (state.mode === "403") return route.fulfill({ status: 403, contentType: "text/plain", body: "Forbidden" });
    if (state.mode === "full") { url.searchParams.delete("since"); return route.fulfill({ status: 200, headers: { "content-type": "application/json" }, body: await (await route.fetch({ url: url.toString() })).text() }); }
    return route.continue();
  });
  await page.goto(ENV.base + "/?t=" + ENV.token, { waitUntil: "load" });
  await settled(page);
  return page;
}
const events = (page, type) => page.evaluate((t) => window.__ev.filter((e) => e.type === t), type);
// The next `semon:polled` after the `n` seen so far.
const nextPolled = async (page, n) => { await page.waitForFunction((k) => window.__ev.filter((e) => e.type === "semon:polled").length > k, n, { timeout: 12000 }); return (await events(page, "semon:polled"))[n]; };

export default async function embedCheck(browser) {
  const r = reporter("embed"), R = r.results;
  r.expect(!!ENV.base && !!ENV.token, "the primary fixture server is required");

  // ---- semon:polled and semon:refresh ----
  {
    const state = { mode: "pass" }, page = await open(browser, { state });
    let n = 0;
    const p304 = await nextPolled(page, n++);
    r.expect(p304.detail?.ok === true, "semon:polled after a 304 should say ok: " + JSON.stringify(p304));
    state.mode = "full";
    const p200 = await nextPolled(page, n++);
    r.expect(p200.detail?.ok === true, "semon:polled after a 200 should say ok: " + JSON.stringify(p200));
    state.mode = "500";
    const p500 = await nextPolled(page, n++);
    r.expect(p500.detail?.ok === false, "semon:polled after a 500 should not say ok: " + JSON.stringify(p500));
    R.polled = [p304.detail, p200.detail, p500.detail];

    // The failure backed the next poll off to 4 s. A refresh polls at once, and the 304 that follows resets the delay to 2 s.
    state.mode = "pass";
    const before = await page.evaluate(() => { const t = performance.now(); window.__count = window.__fetches.length; dispatchEvent(new Event("semon:refresh")); return t; });
    await page.waitForFunction((k) => window.__fetches.length > k, await page.evaluate(() => window.__count), { timeout: 3000 }).catch(() => {});
    const fetches = await page.evaluate(() => window.__fetches.slice(window.__count));
    R.refreshDelay = fetches.length ? Math.round(fetches[0] - before) : null;
    r.expect(fetches.length > 0 && fetches[0] - before <= 100, "semon:refresh during a backoff should poll within 100 ms: " + R.refreshDelay);
    await page.waitForFunction((k) => window.__fetches.length > k + 1, await page.evaluate(() => window.__count), { timeout: 3500 }).catch(() => {});
    const after = await page.evaluate(() => window.__fetches.slice(window.__count));
    R.afterRefresh = after.length > 1 ? Math.round(after[1] - after[0]) : null;
    r.expect(after.length > 1 && after[1] - after[0] < 3000, "the poll after a refresh should come 2 s later, not at the backed-off delay: " + R.afterRefresh);
    r.expect(page.errors.length === 0, "page errors: " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- semon:ended ----
  for (const cancel of [true, false]) {
    const page = await open(browser, { state: { mode: "403" } });
    await page.evaluate((c) => { window.__cancel = c; }, cancel);
    await page.waitForFunction(() => window.__ev.some((e) => e.type === "semon:ended"), null, { timeout: 8000 });
    await sleep(300);
    const [ended] = await events(page, "semon:ended"), note = await page.evaluate(() => document.querySelector(".livenote")?.textContent ?? null);
    const tag = cancel ? "cancelled" : "not cancelled";
    R["ended " + tag] = { detail: ended.detail, cancelable: ended.cancelable, note };
    r.expect(ended.cancelable === true && ended.detail?.status === 403, "semon:ended (" + tag + ") should be cancelable and carry the 403: " + JSON.stringify(ended));
    r.expect(cancel ? note === null : note === ENDED, "semon:ended " + tag + ": the note read " + JSON.stringify(note));
    const polls = await page.evaluate(() => window.__fetches.length); await sleep(2500);
    r.expect((await page.evaluate(() => window.__fetches.length)) === polls, "polling should stop after a 403 (" + tag + ")");
    r.expect(page.errors.length === 0, "page errors (" + tag + "): " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- The account menu from the embedding page ----
  const menuOf = (page) => page.evaluate(() => {
    const menu = document.querySelector("#topbar .account-popover");
    return menu ? { name: menu.querySelector(".account-name")?.textContent, login: menu.querySelector(".account-login-value")?.textContent, bold: menu.querySelectorAll("b").length, hrefs: [...menu.querySelectorAll("a[href]")].map((a) => a.getAttribute("href")), actions: [...menu.querySelectorAll("form")].map((f) => f.getAttribute("action")), workspaces: [...menu.querySelectorAll(".account-workspace-name")].map((n) => n.textContent) } : null;
  });
  const openMenu = async (page) => { await page.locator("#topbar .account-avatar-button").click(); return menuOf(page); };
  {
    const page = await open(browser, { embed: { account: account() } });
    const menu = await openMenu(page);
    R.embedMenu = menu;
    r.expect(menu?.name === "Embed <b>Ada</b> Lovelace" && menu.login === "ada" && menu.bold === 0, "the embedding page's account should show as text: " + JSON.stringify(menu));
    r.expect(menu && menu.hrefs.join() === "/embed/profile" && menu.actions.join() === "/embed/switch/1,/embed/sign-out" && menu.workspaces.join() === "Engine room", "the embedding page's menu links: " + JSON.stringify(menu));
    await page.screenshot({ path: path.join(OUT, "embed-menu.png") });
    r.expect(page.errors.length === 0, "page errors (embed menu): " + page.errors.join("; "));
    await page.context().close();
  }
  for (const [what, over] of [
    ["a javascript: link", { links: [{ label: "Bad", href: "javascript:alert(1)", method: "get", danger: false }] }],
    ["a protocol-relative link", { links: [{ label: "Bad", href: "//evil.example/x", method: "get", danger: false }] }],
    ["a backslash link", { links: [{ label: "Bad", href: "/ok\\evil", method: "get", danger: false }] }],
    ["a workspace switch to //host", { workspaces: [{ name: "Bad", role: "", current: true, switch_href: "//evil.example/switch" }] }],
    ["a javascript: avatar", { avatar_href: "javascript:alert(1)" }],
  ]) {
    const page = await open(browser, { embed: { account: account(over) } });
    const widgets = await page.locator(".account-widget").count(), bad = await page.locator('a[href^="javascript:"], a[href^="//"], form[action^="//"], img[src^="javascript:"]').count();
    R["invalid " + what] = { widgets, bad };
    r.expect(widgets === 0 && bad === 0, "the embedding page's account with " + what + " should be dropped like a server's, and nothing built from it: " + JSON.stringify({ widgets, bad }));
    await page.context().close();
  }
  {
    const server = account({ name: "Server Grace", login: "grace", initials: "SG", workspaces: [{ name: "Bridge", role: "admin", current: true, switch_href: "/server/switch" }], links: [{ label: "Server profile", href: "/server/profile", method: "get", danger: false }] });
    const page = await open(browser, { embed: { account: account() }, serverAccount: server });
    const menu = await openMenu(page);
    R.serverWins = menu;
    r.expect(menu?.name === "Server Grace" && menu.hrefs.join() === "/server/profile", "a server-provided account should win over the embedding page's: " + JSON.stringify(menu));
    r.expect(page.errors.length === 0, "page errors (server wins): " + page.errors.join("; "));
    await page.context().close();
    // A server account the rules reject leaves the embedding page's.
    const invalid = { ...server, links: [{ label: "Bad", href: "//evil.example/x", method: "get", danger: false }] };
    const fallback = await open(browser, { embed: { account: account() }, serverAccount: invalid });
    const menu2 = await openMenu(fallback);
    R.serverInvalid = menu2;
    r.expect(menu2?.name === "Embed <b>Ada</b> Lovelace", "an invalid server account should leave the embedding page's: " + JSON.stringify(menu2));
    await fallback.context().close();
  }

  return r.done();
}

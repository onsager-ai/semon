// The tooltip (data-tip, tooltip.js) and the end of the native `title`:
//   - no rendered screen has an element with a title attribute, or an SVG <title>: every session, a trace, Home, Sessions,
//     Machines, a machine, Analytics at each range and its slice dialog, the ⋯ menu, the runs sheet, the details sheet, the
//     collapsed rail and the phone drawer, at 390 and 1280 px;
//   - on the shell gallery and on the served viewer's session top bar, light and dark, 390 and 1280 px:
//       hover opens it only after 400 ms or more (500 ms is the delay), and a neighbour reached at once shows at once;
//       leaving closes it and brings the delay back after the 300 ms window; focus (keyboard) shows it with no delay,
//       Esc hides it, a click hides it and keeps it hidden until the pointer leaves; aria-describedby is set while it shows
//       and given back after; one tooltip element for the whole page; a scroll that moves the target closes it; a changed
//       data-tip is followed and a removed target closes it; it stays 8 px inside the viewport and off the target; its text
//       meets AA against its background; reduced motion turns the fade off;
//       on a phone, tapping a static tipped element toggles it, tapping elsewhere closes it, and tapping a control runs the
//       control with no tip.
// Screenshots with a tooltip open go to out/tooltip/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, VIEWPORTS, served, goto, data, reporter } from "../lib.mjs";

const OUT = path.join(ENV.out, "tooltip");
fs.mkdirSync(OUT, { recursive: true });
const GALLERY = new URL("../shell-gallery.html", import.meta.url);
const GALLERY_PATH = "/__tooltip-gallery.html";

// A page of the shell gallery on the viewer's origin, as shell.mjs opens it: its stylesheet first, for the token cookie and
// the security headers, then the page answered from the file with those headers.
async function galleryPage(browser, { size, dark, reduced = false }) {
  const context = await browser.newContext({ ...VIEWPORTS[size], colorScheme: dark ? "dark" : "light", timezoneId: "UTC", locale: "en-US", reducedMotion: reduced ? "reduce" : "no-preference" });
  const page = await context.newPage();
  page.errors = [];
  page.on("pageerror", (error) => page.errors.push(error.message.split("\n")[0]));
  page.setDefaultTimeout(5000);
  await page.route(/.*/, (route) => (route.request().url().startsWith(ENV.base + "/") ? route.continue() : route.abort()));
  const sheet = await page.goto(ENV.base + "/shell.css?t=" + ENV.token, { waitUntil: "load" });
  const skip = ["content-type", "content-length", "content-encoding", "etag", "set-cookie", "date", "connection", "transfer-encoding", "keep-alive"];
  const headers = Object.fromEntries(Object.entries(sheet.headers()).filter(([name]) => !skip.includes(name)));
  const body = fs.readFileSync(GALLERY, "utf8");
  await page.route(ENV.base + GALLERY_PATH, (route) => route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", headers, body }));
  await page.goto(ENV.base + GALLERY_PATH, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
  return page;
}

// Resolves with the milliseconds until the tooltip is shown, or null when it is not within `ms`.
const PROBE = (ms) => new Promise((resolve) => {
  const start = performance.now();
  const tick = () => {
    const tip = document.getElementById("sh-tooltip");
    if (tip && !tip.hidden) return resolve(performance.now() - start);
    if (performance.now() - start > ms) return resolve(null);
    requestAnimationFrame(tick);
  };
  tick();
});
const state = (page) => page.evaluate(() => {
  const tip = document.getElementById("sh-tooltip");
  const open = !!tip && !tip.hidden;
  const rect = open ? tip.getBoundingClientRect() : null;
  return {
    open, text: open ? tip.textContent : null, side: open ? tip.dataset.side : null, role: tip?.getAttribute("role") ?? null, count: document.querySelectorAll('[role="tooltip"]').length,
    rect: rect && { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom },
    view: { width: document.documentElement.clientWidth, height: document.documentElement.clientHeight },
    described: [...document.querySelectorAll('[aria-describedby~="sh-tooltip"]')].length,
  };
});
const centre = (box) => [box.x + box.width / 2, box.y + box.height / 2];
const box = async (page, selector) => { const b = await page.locator(selector).first().boundingBox(); if (!b) throw new Error("no box for " + selector); return b; };
// Moves the pointer to an element's centre and reports how long the tooltip took (null: never, within `ms`).
async function hover(page, target, ms = 1400) {
  const probe = page.evaluate(PROBE, ms);
  const b = typeof target === "string" ? await box(page, target) : target;
  await page.mouse.move(...centre(b));
  return probe;
}
// A corner of the viewport that holds nothing tipped: the pointer leaves whatever it was over.
const away = async (page) => { const v = page.viewportSize(); await page.mouse.move(v.width - 2, v.height - 2); };
// Puts the gallery's tooltip section in the middle of the screen, so its elements have room above and below.
const reveal = async (page) => { await page.evaluate(() => document.getElementById("tip-static").scrollIntoView({ block: "center" })); await page.waitForTimeout(150); };

// WCAG contrast of the open tooltip's text on its background.
const contrast = (page) => page.evaluate(() => {
  const tip = document.getElementById("sh-tooltip"), cs = getComputedStyle(tip);
  const parse = (css) => css.match(/[\d.]+/g).slice(0, 4).map(Number);
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  const fg = parse(cs.color), bg = parse(cs.backgroundColor), a = lum(fg), b = lum(bg);
  return { ratio: Math.round((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05) * 100) / 100, alpha: bg[3] ?? 1, fontSize: parseFloat(cs.fontSize), maxWidth: cs.maxWidth, animation: cs.animationName };
});

const titles = (page) => page.evaluate(() => [
  ...[...document.querySelectorAll("[title]")].map((e) => e.tagName.toLowerCase() + (typeof e.className === "string" && e.className ? "." + e.className.split(" ")[0] : "") + '[title="' + e.getAttribute("title").slice(0, 40) + '"]'),
  ...[...document.querySelectorAll("svg title")].map((e) => "svg <title>" + e.textContent.slice(0, 40)),
]);

// Inside the viewport by 8 px, and off the target.
function placement(tag, what, s, target, r) {
  if (!s.open) { r.expect(false, tag + " " + what + ": the tooltip is not open"); return; }
  const m = 8 - 0.5;
  r.expect(s.rect.left >= m && s.rect.right <= s.view.width - m && s.rect.top >= m && s.rect.bottom <= s.view.height - m, tag + " " + what + ": the tooltip leaves the 8 px margin " + JSON.stringify({ rect: s.rect, view: s.view }));
  const apart = s.rect.bottom <= target.y + 0.5 || s.rect.top >= target.y + target.height - 0.5 || s.rect.right <= target.x + 0.5 || s.rect.left >= target.x + target.width - 0.5;
  r.expect(apart, tag + " " + what + ": the tooltip covers its target " + JSON.stringify({ rect: s.rect, target }));
}

const OPEN_MS = 400;
async function behaviour(page, tag, r, rec, { first, second, third }) {
  // Opens only after the delay.
  await away(page); await page.waitForTimeout(450);
  const early = await hover(page, first, 1500);
  r.expect(early != null && early >= OPEN_MS && early < 1400, tag + ": hover opened the tooltip after " + early + " ms, expected 400 to 1400");
  const s1 = await state(page);
  rec.hoverMs = early == null ? null : Math.round(early);
  r.expect(s1.open && s1.role === "tooltip" && s1.count === 1 && !!s1.text, tag + ": the tooltip is not one role=tooltip element with text " + JSON.stringify(s1));
  // Moving to a neighbour within the window shows it at once.
  const skipped = await hover(page, second, 1000);
  const s2 = await state(page);
  r.expect(skipped != null && skipped < OPEN_MS - 100, tag + ": a neighbour took " + skipped + " ms, expected it at once");
  r.expect(s2.open && s2.text !== s1.text && s2.count === 1, tag + ": the neighbour did not replace the tooltip's text: " + JSON.stringify([s1.text, s2.text]));
  rec.neighbourMs = skipped == null ? null : Math.round(skipped);
  // Leaving closes it.
  await away(page); await page.waitForTimeout(120);
  r.expect(!(await state(page)).open, tag + ": leaving did not close the tooltip");
  // After the window, the delay is back.
  await page.waitForTimeout(450);
  const later = await hover(page, third ?? first, 1500);
  r.expect(later != null && later >= OPEN_MS, tag + ": after the skip window the delay was " + later + " ms");
  const s3 = await state(page);
  // aria-describedby while it shows, given back after.
  r.expect(s3.described === 1, tag + ": aria-describedby is set on " + s3.described + " elements while a tooltip shows, expected 1");
  await away(page); await page.waitForTimeout(150);
  r.expect((await state(page)).described === 0, tag + ": aria-describedby stayed after the tooltip closed");
  // A click hides it and keeps it hidden until the pointer leaves.
  await page.waitForTimeout(450);
  await hover(page, first, 1500);
  await page.mouse.down(); await page.mouse.up(); await page.waitForTimeout(120);
  r.expect(!(await state(page)).open, tag + ": a click did not hide the tooltip");
  await page.waitForTimeout(700);
  r.expect(!(await state(page)).open, tag + ": the tooltip came back after a click without the pointer leaving");
  await away(page); await page.waitForTimeout(450);
}

// Runs one section; a throw is a failure of that section, so the rest of the report is still written. Closes the page's context.
async function guard(r, name, page, fn) {
  try { await fn(); } catch (e) { r.expect(false, name + ": threw " + String(e?.message ?? e).split("\n")[0]); }
  finally { await page.context().close().catch(() => {}); }
}

export default async function tooltipCheck(browser) {
  const D = await data(), r = reporter("tooltip"), results = {};
  const parent = Object.values(D.SESS).find((s) => s.name === "harbor") ?? Object.values(D.SESS).find((s) => D.H.some((h) => h.kind === "spawn" && h.from === s.id));
  r.expect(!!parent, "the fixture must hold a session with child runs");

  // ---- The shell gallery ----
  for (const [size, dark] of [["desktop", false], ["desktop", true], ["phone", false], ["phone", true]]) {
    const tag = "gallery " + size + (dark ? "-dark" : "-light"), rec = (results[tag] = {}), page = await galleryPage(browser, { size, dark });
    await guard(r, tag, page, async () => {
      await reveal(page);
      await behaviour(page, tag, r, rec, { first: "#tip-static", second: "#tip-static-2", third: "#tip-static" });
      // Placement, contrast, size.
      await reveal(page);
      await hover(page, "#tip-button", 1500);
      const s = await state(page), b = await box(page, "#tip-button");
      placement(tag, "button", s, b, r);
      const c = await contrast(page);
      rec.contrast = c;
      r.expect(c.ratio >= 4.5 && c.alpha === 1, tag + ": the tooltip's contrast is " + c.ratio + " (alpha " + c.alpha + "), under 4.5");
      r.expect(c.fontSize === 13 && c.maxWidth === Math.min(280, s.view.width - 16) + "px", tag + ": the tooltip is " + c.fontSize + "px, max-width " + c.maxWidth);
      r.expect(c.animation === "tip-in" || c.animation === "none", tag + ": unexpected animation " + c.animation);
      await page.waitForTimeout(300); // past the 120 ms fade
      await page.screenshot({ path: path.join(OUT, "tip-gallery-" + size + (dark ? "-dark" : "-light") + ".png") });
      await away(page); await page.waitForTimeout(450);
      // A scroll that moves the target closes it.
      await hover(page, "#tip-static", 1500);
      // The gallery may already stand at the end of its scroll, so try down and then up.
      const moved = await page.evaluate(() => { const t = document.getElementById("tip-static"), before = t.getBoundingClientRect().top, main = document.getElementById("main"); for (const delta of [40, -40]) { if (main && main.scrollHeight > main.clientHeight) main.scrollTop += delta; window.scrollBy(0, delta); if (Math.abs(t.getBoundingClientRect().top - before) > 1) return true; } return false; });
      await page.waitForTimeout(150);
      r.expect(moved, tag + ": the gallery could not be scrolled to move the target");
      r.expect(!(await state(page)).open, tag + ": a scroll that moved the target did not close the tooltip");
      await away(page); await page.waitForTimeout(450);
      // A changed data-tip is followed; a removed target closes it.
      await reveal(page);
      await hover(page, "#tip-static-2", 1500);
      await page.evaluate(() => { document.getElementById("tip-static-2").dataset.tip = "The text changed while it showed"; });
      await page.waitForTimeout(150);
      r.expect((await state(page)).text === "The text changed while it showed", tag + ": an open tooltip did not follow a changed data-tip");
      await page.evaluate(() => document.getElementById("tip-static-2").remove());
      await page.waitForTimeout(150);
      r.expect(!(await state(page)).open, tag + ": removing the target left its tooltip open");
      if (size === "phone") {
        await reveal(page);
        await page.touchscreen.tap(...centre(await box(page, "#tip-static"))); await page.waitForTimeout(120);
        const first = await state(page);
        r.expect(first.open && /^A static badge/.test(first.text), tag + ": tapping the static badge did not show its tooltip " + JSON.stringify(first));
        await page.touchscreen.tap(...centre(await box(page, "#tip-static"))); await page.waitForTimeout(120);
        r.expect(!(await state(page)).open, tag + ": tapping the badge again did not close it");
        await page.touchscreen.tap(...centre(await box(page, "#tip-static"))); await page.waitForTimeout(120);
        await page.touchscreen.tap(...centre(await box(page, "#tip-button"))); await page.waitForTimeout(120);
        r.expect(!(await state(page)).open, tag + ": tapping a button showed a tooltip");
        await page.touchscreen.tap(...centre(await box(page, "#tip-static"))); await page.waitForTimeout(120);
        await page.touchscreen.tap(4, 300); await page.waitForTimeout(120);
        r.expect(!(await state(page)).open, tag + ": tapping elsewhere did not close the tooltip");
      }
      r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
    });
  }
  // Reduced motion: no fade. Motion allowed: the fade, unless a neighbour's tip was just open.
  for (const reduced of [true, false]) {
    const tag = "gallery motion " + (reduced ? "reduced" : "allowed"), page = await galleryPage(browser, { size: "desktop", dark: false, reduced });
    await guard(r, tag, page, async () => {
      await reveal(page); await hover(page, "#tip-static", 1500);
      const c = await contrast(page);
      results[tag] = c.animation;
      r.expect(reduced ? c.animation === "none" : c.animation === "tip-in", tag + ": the animation is " + c.animation);
    });
  }
  // Keyboard focus, on the gallery: Tab to the icon, and to the button.
  {
    const tag = "gallery keyboard", page = await galleryPage(browser, { size: "desktop", dark: false });
    await guard(r, tag, page, async () => {
      const seen = [];
      for (let i = 0; i < 80; i++) {
        await page.keyboard.press("Tab");
        const at = await page.evaluate(() => document.activeElement?.id ?? "");
        if (at === "tip-icon" || at === "tip-button") {
          await page.waitForTimeout(60);
          const s = await state(page), want = await page.evaluate(() => document.activeElement.dataset.tip);
          r.expect(s.open && s.text === want, tag + ": focus on #" + at + " did not show its tooltip at once " + JSON.stringify(s));
          seen.push(at);
          if (at === "tip-icon") { await page.keyboard.press("Escape"); await page.waitForTimeout(80); r.expect(!(await state(page)).open, tag + ": Esc did not hide the tooltip"); }
        }
        if (seen.length === 2) break;
      }
      r.expect(seen.length === 2, tag + ": Tab reached only " + JSON.stringify(seen));
      results[tag] = seen;
    });
  }

  // ---- The viewer's session top bar ----
  for (const [size, dark] of [["desktop", false], ["desktop", true], ["phone", false], ["phone", true]]) {
    const tag = "bar " + size + (dark ? "-dark" : "-light"), rec = (results[tag] = {}), page = await served(browser, { size, dark });
    await guard(r, tag, page, async () => {
      await goto(page, { v: "session", id: parent.id }, D);
      await page.waitForTimeout(250);
      // The state badge holds the errors control, so it is reached by its dot; the rest are the line's own children.
      const items = await page.evaluate(() => [...document.querySelectorAll("#topbar .l2.session-meta > [data-tip]")].filter((n) => !n.classList.contains("meta-state") && n.getClientRects().length && n.getBoundingClientRect().width > 0).map((n, i) => { n.dataset.probe = String(i); return { i, tip: n.dataset.tip, cls: n.className, tag: n.tagName }; }));
      rec.items = items.map((x) => x.cls.replace(/meta-item ?/, "") || x.tag + ":" + x.tip.slice(0, 20));
      r.expect(items.length >= (size === "desktop" ? 4 : 0), tag + ": only " + items.length + " tipped items in the session line: " + JSON.stringify(rec.items));
      const at = (i) => box(page, '#topbar [data-probe="' + i + '"]');
      const dotBox = await box(page, "#topbar .meta-state > .dot"), reach = [dotBox, ...(await Promise.all(items.slice(0, 3).map((x) => at(x.i))))];
      if (reach.length >= 2) await behaviour(page, tag, r, rec, { first: reach[0], second: reach[1] });
      // Every visible tipped item in the bar and the sidebar stays inside the margin and off its target.
      const spots = await page.evaluate(() => { const list = [...document.querySelectorAll("#topbar [data-tip], #sidebar [data-tip]")].filter((n) => { const b = n.getBoundingClientRect(); return n.getClientRects().length && b.width > 0 && b.left >= 0 && b.right <= innerWidth && b.top >= 0 && b.bottom <= innerHeight && (!n.hasAttribute("data-tip-clipped") || n.scrollWidth > n.clientWidth + 1); }); return list.slice(0, 40).map((n, i) => { n.dataset.spot = String(i); return { i, tip: n.dataset.tip, clipped: n.hasAttribute("data-tip-clipped") }; }); });
      let checked = 0;
      await away(page); await page.waitForTimeout(450);
      for (const spot of spots) {
        await away(page);
        const b = await box(page, '[data-spot="' + spot.i + '"]'), t = await hover(page, b, 1500);
        if (t == null) { if (!spot.clipped) r.expect(false, tag + ": " + JSON.stringify(spot.tip.slice(0, 40)) + " never showed"); continue; }
        const s = await state(page); placement(tag, JSON.stringify(spot.tip.slice(0, 30)), s, b, r); checked++;
      }
      rec.placed = checked;
      // A name that fits has no tip (data-tip-clipped): hovering it opens nothing.
      const whole = await page.evaluate(() => { const n = [...document.querySelectorAll("#sidebar .srow .nm[data-tip-clipped]")].find((x) => { const b = x.getBoundingClientRect(); return b.width > 0 && b.right <= innerWidth && b.left >= 0 && x.scrollWidth <= x.clientWidth + 1; }); if (!n) return null; n.dataset.whole = ""; return true; });
      if (whole) { await away(page); await page.waitForTimeout(450); const t = await hover(page, "#sidebar .nm[data-whole]", 800); r.expect(t == null, tag + ": a name that is not cut off showed a tooltip after " + t + " ms"); }
      rec.whole = !!whole;
      r.expect(checked >= (size === "desktop" ? 5 : 1), tag + ": only " + checked + " tooltips were placed");
      // A long tip (the cost badge, or the state), for the contrast, the size and the screenshot.
      await away(page); await page.waitForTimeout(450);
      const shot = (await page.locator("#topbar .meta-cost").count()) && await page.locator("#topbar .meta-cost").first().isVisible() ? "#topbar .meta-cost" : "#topbar .meta-state";
      await hover(page, shot, 1500);
      const c = await contrast(page); rec.contrast = c.ratio;
      r.expect(c.ratio >= 4.5 && c.alpha === 1, tag + ": the tooltip's contrast is " + c.ratio + ", under 4.5");
      await page.waitForTimeout(300); // past the 120 ms fade
      await page.screenshot({ path: path.join(OUT, "tip-bar-" + size + (dark ? "-dark" : "-light") + ".png"), clip: { x: 0, y: 0, width: VIEWPORTS[size].viewport.width, height: 220 } });
      await away(page); await page.waitForTimeout(450);

      if (size === "phone") {
        // Touch: a static badge toggles its tip; elsewhere closes it; a control runs and shows none.
        const state0 = await box(page, "#topbar .meta-state > .dot"), blank = [4, 400];
        await page.touchscreen.tap(...centre(state0)); await page.waitForTimeout(120);
        const opened = await state(page);
        r.expect(opened.open && /^Status: /.test(opened.text), tag + ": tapping the state badge did not show its tooltip " + JSON.stringify(opened));
        await page.touchscreen.tap(...centre(state0)); await page.waitForTimeout(120);
        r.expect(!(await state(page)).open, tag + ": tapping the badge again did not close its tooltip");
        await page.touchscreen.tap(...centre(state0)); await page.waitForTimeout(120);
        await page.touchscreen.tap(...blank); await page.waitForTimeout(120);
        r.expect(!(await state(page)).open, tag + ": tapping elsewhere did not close the tooltip");
        const runs = page.locator("#topbar .meta-runs");
        if (await runs.count() && await runs.first().isVisible()) {
          await page.touchscreen.tap(...centre(await box(page, "#topbar .meta-runs"))); await page.waitForTimeout(250);
          const s = await state(page), dialog = await page.evaluate(() => !!document.querySelector("dialog.runs-sheet[open]"));
          r.expect(!s.open && dialog, tag + ": tapping the runs control showed a tooltip or did not run: " + JSON.stringify({ open: s.open, dialog }));
          await page.keyboard.press("Escape"); await page.waitForTimeout(150);
        }
        // A phone hover-less tap on a tipped control (the collapse toggle is hidden on phones; the drawer's toggle is not tipped).
      }
      r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
    });
  }

  // ---- Keyboard focus on the viewer: Analytics' metric cards, whose explanation is only in the tip ----
  {
    const tag = "analytics keyboard", page = await served(browser, { size: "desktop" });
    await guard(r, tag, page, async () => {
      await goto(page, { v: "analytics" }, D); await page.waitForTimeout(150);
      let reached = false;
      for (let i = 0; i < 160 && !reached; i++) {
        await page.keyboard.press("Tab");
        reached = await page.evaluate(() => document.activeElement?.classList.contains("analytics-metric") ?? false);
      }
      r.expect(reached, tag + ": Tab never reached a metric card");
      if (reached) {
        await page.waitForTimeout(80);
        const s = await state(page), want = await page.evaluate(() => document.activeElement.dataset.tip);
        r.expect(s.open && s.text === want && s.described === 1, tag + ": focusing a card did not show its tooltip and describe it " + JSON.stringify(s));
        await page.keyboard.press("Escape"); await page.waitForTimeout(80);
        const after = await state(page);
        r.expect(!after.open && after.described === 0, tag + ": Esc did not hide the card's tooltip " + JSON.stringify(after));
      }
      results[tag] = reached;
    });
  }

  // ---- No title attribute on any rendered screen ----
  const found = {};
  for (const size of ["desktop", "phone"]) {
    const page = await served(browser, { size });
    await guard(r, "titles " + size, page, async () => {
      const scan = async (name) => { const list = await titles(page); results["titles " + size + " " + name] = list.length; if (list.length) found[size + " " + name] = list.slice(0, 5); };
      for (const route of [{ v: "home" }, { v: "sessions" }, { v: "machines" }, { v: "analytics" }]) { await goto(page, route, D); await page.waitForTimeout(80); await scan(route.v); }
      for (const days of [7, 30]) { await page.click('[data-e="analytics-range:' + days + '"]'); await page.waitForTimeout(150); await scan("analytics-" + days + "d"); }
      const hit = page.locator(".chart-hit[role=button]");
      if (await hit.count()) { await hit.first().dispatchEvent("click"); await page.waitForSelector("dialog.analytics-slice[open]"); await scan("analytics-slice"); await page.keyboard.press("Escape"); await page.waitForTimeout(100); }
      for (const m of Object.keys(D.MACHINE)) { await goto(page, { v: "machine", id: m }, D); await page.waitForTimeout(60); await scan("machine-" + m); }
      const traces = [];
      for (const s of Object.values(D.SESS)) {
        await goto(page, { v: "session", id: s.id }, D); await page.waitForTimeout(80); await scan("session-" + s.name);
        if (traces.length < 3) { const t = await page.evaluate(() => document.querySelector(".turn-end .tracebtn")?.closest(".turn")?.dataset.turn ?? null); if (t) traces.push([s.id, t]); }
      }
      for (const [sid, turn] of traces) { await goto(page, { v: "trace", sid, turn }, D); await page.waitForTimeout(100); await scan("trace-" + turn); }
      // The states around a session: the ⋯ menu, the details sheet, the runs sheet.
      await goto(page, { v: "session", id: parent.id }, D); await page.waitForTimeout(150);
      await page.click("#more-btn"); await page.waitForSelector(".session-menu"); await scan("session-menu");
      await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click(); await page.waitForSelector("dialog.session-details[open]"); await scan("session-details");
      await page.keyboard.press("Escape"); await page.waitForTimeout(150);
      if (await page.locator("#topbar .meta-runs").count() && await page.locator("#topbar .meta-runs").first().isVisible()) {
        await page.locator("#topbar .meta-runs").first().click();
        // A sheet on a phone; a popover in the bar on a desktop.
        const runs = size === "phone" ? "dialog.runs-sheet[open]" : ".runs-popover";
        await page.waitForSelector(runs); await scan("runs-" + (size === "phone" ? "sheet" : "popover"));
        await page.keyboard.press("Escape"); await page.waitForTimeout(150);
        await goto(page, { v: "session", id: parent.id }, D); await page.waitForTimeout(150);
      }
      if (size === "desktop") { await page.click("#rail-toggle"); await page.waitForTimeout(250); await scan("rail"); }
      else { await page.click("#lead-btn"); await page.waitForTimeout(320); await scan("drawer"); }
      r.expect(page.errors.length === 0, "titles " + size + ": page errors " + page.errors.join("; "));
    });
  }
  r.expect(Object.keys(found).length === 0, "elements still have a title attribute or an SVG <title>: " + JSON.stringify(found));

  r.results = results;
  return r.done();
}

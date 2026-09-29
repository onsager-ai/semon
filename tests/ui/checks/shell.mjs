// The gallery is inserted into a document loaded from the viewer's origin so its absolute asset paths, CSP, and fonts
// are exercised exactly as they are for an ordinary page served beside the viewer.
import fs from "node:fs";
import path from "node:path";
import { ENV, reporter, served, contrastOf } from "../lib.mjs";

const pages = [
  ["shell-gallery", new URL("../shell-gallery.html", import.meta.url)],
  ["shell-signin", new URL("../shell-signin.html", import.meta.url)],
];
const sizes = [
  ["390", 390, 844, true],
  ["1280", 1280, 800, false],
];
const schemes = ["light", "dark"];
const output = path.join(ENV.out, "shell");
const GALLERY_PATH = "/__shell-gallery.html";
const PHONE_TARGET_SELECTOR = ".btn, .field input, .code .copy, dialog.sheet button, .sh-select-trigger, .sh-select-option, .sh-select-close";
fs.mkdirSync(output, { recursive: true });

async function galleryPage(browser, html, width, height, mobile, scheme) {
  const context = await browser.newContext({
    viewport: { width, height },
    deviceScaleFactor: mobile ? 2 : 1,
    isMobile: mobile,
    hasTouch: mobile,
    colorScheme: scheme,
    timezoneId: "UTC",
  });
  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: ENV.base });
  const page = await context.newPage();
  page.errors = [];
  page.on("pageerror", (error) => page.errors.push(error.message.split("\n")[0]));
  page.setDefaultTimeout(5000);
  await page.route(/.*/, (route) => (
    route.request().url().startsWith(ENV.base + "/") ? route.continue() : route.abort()
  ));
  // Loading the stylesheet with the token sets the server cookie and shows which security headers the viewer's origin sends.
  const stylesheet = await page.goto(ENV.base + "/shell.css?t=" + ENV.token, { waitUntil: "load" });
  // The gallery is then navigated to as a page of its own on that origin, answered from the file with those same headers
  // (CSP included), rather than written into the stylesheet's document with page.setContent. Two CI runs (36483037109 and
  // 36528320379) hung in setContent's wait for "load" on the first gallery page, the second after a 30 s timeout, so the
  // wait was not merely slow. The cause was not established (no browser to reproduce it here); a navigation does not
  // go through document.open() on the just-loaded stylesheet document, and serves the same page under the same origin,
  // headers and cookie.
  const headers = Object.fromEntries(Object.entries(stylesheet.headers())
    .filter(([name]) => !["content-type", "content-length", "content-encoding", "etag", "set-cookie", "date", "connection", "transfer-encoding", "keep-alive"].includes(name)));
  await page.route(ENV.base + GALLERY_PATH, (route) => route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", headers, body: html }));
  await page.goto(ENV.base + GALLERY_PATH, { waitUntil: "load" });
  await page.emulateMedia({ colorScheme: scheme });
  await page.evaluate(() => document.fonts.ready);
  return { context, page };
}

const geometry = (page) => page.evaluate(() => {
  const selector = (element) => element.id
    ? "#" + CSS.escape(element.id)
    : element.tagName.toLowerCase() + [...element.classList].map((name) => "." + CSS.escape(name)).join("");
  const width = document.documentElement.clientWidth;
  const right = [];
  for (const element of document.querySelectorAll("*")) {
    const box = element.getBoundingClientRect();
    if (box.width && box.height && box.right > width + 0.5) right.push({ selector: selector(element), right: Math.round(box.right * 10) / 10 });
  }
  const smallInputs = [...document.querySelectorAll("input")]
    .filter((element) => element.getClientRects().length && Number.parseFloat(getComputedStyle(element).fontSize) < 16)
    .map(selector);
  return {
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth,
    right,
    smallInputs,
  };
});

const phoneTargets = (page) => page.evaluate((targetSelector) => {
  const selector = (element) => element.id
    ? "#" + CSS.escape(element.id)
    : element.tagName.toLowerCase() + [...element.classList].map((name) => "." + CSS.escape(name)).join("");
  return [...document.querySelectorAll(targetSelector)]
    .filter((element) => element.getClientRects().length && getComputedStyle(element).visibility !== "hidden")
    .map((element) => ({ selector: selector(element), height: Math.round(element.getBoundingClientRect().height * 10) / 10 }));
}, PHONE_TARGET_SELECTOR);

// Polls a selector's bounding rect (rounded to a tenth of a pixel) until it holds for three consecutive animation
// frames, or a timeout passes. Used so a screenshot or an assertion never lands mid-transition: a CSS transition
// (the drawer's slide, in this codebase) keeps changing the rect every frame until it finishes, so waiting for it
// to stop moving is equivalent to waiting for the transition to end, without depending on a "transitionend" event
// firing for the right property (or at all, if a browser coalesces or skips it).
const stableRect = (page, selector, timeout = 1500) => page.evaluate(({ selector, timeout }) => new Promise((resolve) => {
  const deadline = performance.now() + timeout;
  let lastKey = null, stableFrames = 0;
  const read = () => {
    const element = document.querySelector(selector);
    if (!element) return null;
    const box = element.getBoundingClientRect();
    return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, width: box.width, height: box.height };
  };
  const step = () => {
    const box = read();
    const key = box ? [box.left, box.top, box.width, box.height].map((n) => Math.round(n * 10)).join(",") : "none";
    if (key === lastKey) stableFrames += 1; else { stableFrames = 0; lastKey = key; }
    if (stableFrames >= 3 || performance.now() > deadline) { resolve(box); return; }
    requestAnimationFrame(step);
  };
  step();
}), { selector, timeout });

const chromeDimensions = (page) => page.evaluate(() => {
  const computed = (element) => {
    if (!element) return null;
    const style = getComputedStyle(element);
    return { width: Number.parseFloat(style.width), height: Number.parseFloat(style.height) };
  };
  return {
    lead: computed(document.querySelector("#lead-btn")),
    nav: computed(document.querySelector(".nav-item")),
  };
});

// The Select in the gallery (select.js): built from the native selects marked data-select, closed and open, by mouse and by
// keyboard; a popover under the button on a desktop, a bottom sheet on a phone. Screenshots: <key>-select-{closed,open,search,flip}.png.
async function selectChecks(page, key, mobile, r, results) {
  const out = (results[key].select = {});
  const room = '.sh-select[data-label="Room"]', device = '.sh-select[data-label="Device"]', trig = (root) => root + " .sh-select-trigger";
  const state = (root) => page.evaluate((root) => {
    const box = document.querySelector(root), t = box.querySelector(".sh-select-trigger"), pop = box.querySelector(".sh-select-pop"), sheet = box.querySelector("dialog.sh-select-sheet");
    const list = box.querySelector('[role="listbox"]'), at = t.getAttribute("aria-activedescendant") ?? list.getAttribute("aria-activedescendant") ?? box.querySelector(".sh-select-search input")?.getAttribute("aria-activedescendant");
    const shown = [...list.querySelectorAll('[role="option"]')].filter((o) => o.getClientRects().length);
    const rect = (n) => { const b = n?.getBoundingClientRect(); return b && b.width ? { left: b.left, top: b.top, right: b.right, bottom: b.bottom, width: b.width, height: b.height } : null; };
    const search = box.querySelector(".sh-select-search input");
    return {
      text: t.textContent.trim(), expanded: t.getAttribute("aria-expanded"), controls: t.getAttribute("aria-controls"), role: t.getAttribute("role"), listId: list.id,
      popOpen: !!pop && !pop.hidden, sheetOpen: !!sheet?.open, focus: document.activeElement === t, searchFocus: !!search && document.activeElement === search,
      active: at ? document.getElementById(at)?.textContent.trim() ?? null : null,
      options: shown.map((o) => ({ text: o.textContent.trim(), selected: o.getAttribute("aria-selected"), height: o.getBoundingClientRect().height, check: getComputedStyle(o.querySelector(".sh-select-check")).visibility })),
      searchShown: !!search && search.getClientRects().length > 0, searchFont: search ? Number.parseFloat(getComputedStyle(search).fontSize) : null,
      empty: box.querySelector(".sh-select-empty")?.getClientRects().length > 0,
      pop: rect(pop), sheet: rect(sheet), side: pop?.dataset.side ?? null, barBottom: document.getElementById("topbar")?.getBoundingClientRect().bottom ?? 0, vw: document.documentElement.clientWidth, vh: innerHeight,
      native: { room: document.getElementById("room-select").value, device: document.getElementById("device-select").value },
    };
  }, root);
  // Whether focus is on the next tabbable control after the root's button (what a Tab from the button reaches).
  const nextFocus = (root) => page.evaluate((root) => {
    const tabbable = [...document.querySelectorAll('a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])')].filter((e) => e.getClientRects().length && getComputedStyle(e).visibility !== "hidden");
    const at = tabbable.indexOf(document.querySelector(root + " .sh-select-trigger")), a = document.activeElement;
    return { onNext: at >= 0 && a === tabbable[at + 1], active: a?.outerHTML.slice(0, 80), next: tabbable[at + 1]?.outerHTML.slice(0, 80) };
  }, root);
  const inside = (b, s) => !!b && b.left >= -0.5 && b.top >= -0.5 && b.right <= s.vw + 0.5 && b.bottom <= s.vh + 0.5;
  const wait = (root, open) => page.waitForFunction(({ root, open }) => { const t = document.querySelector(root + " .sh-select-trigger"); return (t.getAttribute("aria-expanded") === "true") === open; }, { root, open });

  const built = await page.evaluate(() => ({ triggers: [...document.querySelectorAll(".sh-select-trigger")].map((t) => t.textContent.trim()), natives: [...document.querySelectorAll("select[data-select]")].every((s) => s.hidden) }));
  out.built = built;
  r.expect(built.triggers.join("|") === "Room: Living room|Device: Router" && built.natives, key + " the native selects weren't replaced by Selects: " + JSON.stringify(built));
  await page.locator('section[aria-labelledby="select-title"]').screenshot({ path: path.join(output, key + "-select-closed.png") });

  // Open by a click.
  await page.evaluate((root) => document.querySelector(root).scrollIntoView({ block: "center" }), room);
  const scrolledTo = await page.evaluate(() => window.scrollY); // the baseline for the phone's scroll lock: the page is scrolled before the sheet opens
  if (mobile) r.expect(scrolledTo > 0, key + " the gallery isn't scrolled before the sheet opens (no baseline for the scroll lock): " + scrolledTo);
  if (mobile) {
    // The probe's control: with no sheet open a wheel does scroll the page (so "it didn't move" below means something).
    await page.mouse.move(195, 300); await page.mouse.wheel(0, 200); await page.waitForTimeout(200);
    const moved = await page.evaluate(() => window.scrollY); out.wheelControl = { from: scrolledTo, to: moved };
    r.expect(moved > scrolledTo, key + " a wheel doesn't scroll the page even with no sheet open, so the lock check proves nothing: " + JSON.stringify(out.wheelControl));
    await page.evaluate((y) => window.scrollTo(0, y), scrolledTo);
  }
  await page.click(trig(room)); await wait(room, true);
  let s = await state(room); out.open = s;
  r.expect(s.role === "combobox" && s.controls === s.listId && s.expanded === "true", key + " the trigger isn't an expanded combobox that controls its listbox: " + JSON.stringify({ role: s.role, controls: s.controls, list: s.listId }));
  r.expect(s.options.length === 5 && s.options[0].text === "All rooms" && s.options.filter((o) => o.selected === "true").map((o) => o.text).join() === "Living room", key + " options or selection are wrong: " + JSON.stringify(s.options));
  r.expect(s.options.filter((o) => o.check === "visible").length === 1 && s.options.find((o) => o.text === "Living room")?.check === "visible", key + " the selected option doesn't carry the only check mark: " + JSON.stringify(s.options));
  r.expect(!s.searchShown, key + " a list of 5 options shows a search field");
  if (mobile) {
    r.expect(s.sheetOpen && !s.popOpen, key + " a phone didn't open a sheet: " + JSON.stringify({ sheet: s.sheetOpen, pop: s.popOpen }));
    r.expect(!!s.sheet && Math.abs(s.sheet.bottom - s.vh) <= 1 && s.sheet.left <= 0.5 && s.sheet.right >= s.vw - 0.5, key + " the sheet isn't at the bottom edge, full width: " + JSON.stringify(s.sheet));
    r.expect(s.options.every((o) => o.height >= 44), key + " sheet rows under 44 px: " + JSON.stringify(s.options.map((o) => o.height)));
    const pad = await page.evaluate(() => Number.parseFloat(getComputedStyle(document.querySelector(".sh-select-sheet .sh-select-list")).paddingBottom));
    r.expect(pad >= 10, key + " the sheet's list has no bottom padding for the safe area: " + pad);
  } else {
    r.expect(s.popOpen && !s.sheetOpen && inside(s.pop, s), key + " the popover isn't open inside the viewport: " + JSON.stringify(s.pop));
    r.expect(s.pop && s.pop.top >= (await page.evaluate((root) => document.querySelector(root + " .sh-select-trigger").getBoundingClientRect().bottom, room)) - 1, key + " the popover isn't under the trigger: " + JSON.stringify(s.pop));
  }
  out.contrast = {};
  for (const [name, selector] of [["label", room + " .sh-select-label"], ["value", room + " .sh-select-value"], ["option", room + " .sh-select-option"], ["active", room + " .sh-select-option.sh-active"]]) {
    const ratio = await contrastOf(page, selector); out.contrast[name] = ratio;
    r.expect(ratio != null && ratio >= 4.5, key + " " + name + " text contrast " + ratio + " is under 4.5 (AA)");
  }
  await page.screenshot({ path: path.join(output, key + "-select-open.png"), fullPage: false });

  if (mobile) {
    // The page behind the sheet doesn't scroll, and the sheet is a history entry: Back closes it and stays on the page.
    const before = await page.evaluate(() => ({ overflow: getComputedStyle(document.documentElement).overflow, y: window.scrollY, entry: history.state?.shSelect ?? null, path: location.pathname }));
    await page.mouse.move(s.vw / 2, 20); await page.mouse.wheel(0, 300); await page.waitForTimeout(200);
    const scrolled = await page.evaluate(() => window.scrollY);
    out.lock = { ...before, scrolledTo, after: scrolled };
    r.expect(before.y === scrolledTo && scrolled === scrolledTo, key + " opening the sheet moved the page, or a wheel over its backdrop scrolled it: " + JSON.stringify(out.lock));
    r.expect(before.entry != null, key + " opening the sheet pushed no history entry: " + JSON.stringify(before));
    await page.goBack(); await wait(room, false); s = await state(room);
    const back = await page.evaluate(() => ({ overflow: getComputedStyle(document.documentElement).overflow, entry: history.state?.shSelect ?? null, path: location.pathname }));
    out.back = { ...back, sheet: s.sheetOpen, text: s.text };
    r.expect(!s.sheetOpen && s.text === "Room: Living room" && back.path === before.path && back.entry == null, key + " Back didn't close the sheet, stay on the page and unlock it: " + JSON.stringify(out.back));
    await page.click(trig(room)); await wait(room, true);
    // Tapping a row picks it and closes the sheet, and focus returns to the button.
    await page.locator(room + " .sh-select-option", { hasText: "Kitchen" }).click();
    await wait(room, false); s = await state(room); out.tapped = s;
    await page.waitForFunction(() => !history.state?.shSelect);
    r.expect(s.text === "Room: Kitchen" && s.native.room === "kitchen" && !s.sheetOpen && s.focus, key + " tapping a row didn't pick it, close the sheet and return focus: " + JSON.stringify({ text: s.text, native: s.native, sheet: s.sheetOpen, focus: s.focus }));
    // The backdrop closes it without a change; Escape does too.
    await page.click(trig(room)); await wait(room, true);
    await page.mouse.click(s.vw / 2, 20); await wait(room, false);
    s = await state(room); r.expect(s.text === "Room: Kitchen" && !s.sheetOpen, key + " a tap above the sheet didn't close it without a change: " + JSON.stringify(s.text));
    await page.click(trig(room)); await wait(room, true); await page.keyboard.press("Escape"); await wait(room, false);
    s = await state(room); r.expect(!s.sheetOpen && s.focus && s.text === "Room: Kitchen", key + " Escape didn't close the sheet and return focus");
    // More than eight options: a search field in the sheet, at 16 px so a phone doesn't zoom.
    await page.click(trig(device)); await wait(device, true); s = await state(device); out.deviceSheet = s;
    r.expect(s.searchShown && s.searchFont >= 16 && s.options.length === 13 && s.options.every((o) => o.height >= 44), key + " the 13-option sheet: " + JSON.stringify({ search: s.searchShown, font: s.searchFont, n: s.options.length }));
    r.expect(inside(s.sheet, s) && s.sheet.height <= s.vh * 0.81, key + " the tall sheet leaves the viewport or is over 80% of it: " + JSON.stringify(s.sheet));
    await page.screenshot({ path: path.join(output, key + "-select-search.png"), fullPage: false });
    // A wheel over the open 13-option list scrolls the list and never the page, at its end too.
    const listBox = await page.locator(device + " .sh-select-list").boundingBox(), readList = () => page.evaluate((root) => ({ top: document.querySelector(root + " .sh-select-list").scrollTop, room: document.querySelector(root + " .sh-select-list").scrollHeight - document.querySelector(root + " .sh-select-list").clientHeight, y: scrollY }), device);
    const w0 = await readList();
    await page.mouse.move(listBox.x + listBox.width / 2, listBox.y + listBox.height / 2); await page.mouse.wheel(0, 150); await page.waitForTimeout(250);
    const w1 = await readList();
    await page.mouse.wheel(0, 4000); await page.waitForTimeout(250); await page.mouse.wheel(0, 500); await page.waitForTimeout(250);
    const w2 = await readList();
    out.listWheel = { w0, w1, w2 };
    r.expect(w0.room > 0 && w1.top > w0.top && w2.top >= w1.top && w1.y === w0.y && w2.y === w0.y, key + " a wheel over the open list didn't scroll only the list: " + JSON.stringify(out.listWheel));
    await page.keyboard.press("Escape"); await wait(device, false);
    return;
  }

  // Keyboard, on the open list (focus stays on the button): Down moves the highlight, a letter jumps, Enter picks.
  r.expect(s.focus && s.active === "Living room", key + " focus or highlight after opening: " + JSON.stringify({ focus: s.focus, active: s.active }));
  await page.keyboard.press("ArrowDown"); s = await state(room); r.expect(s.active === "Kitchen", key + " ArrowDown highlighted " + s.active);
  await page.keyboard.press("End"); s = await state(room); r.expect(s.active === "Garage", key + " End highlighted " + s.active);
  await page.keyboard.press("Home"); s = await state(room); r.expect(s.active === "All rooms", key + " Home highlighted " + s.active);
  await page.keyboard.press("o"); s = await state(room); r.expect(s.active === "Office", key + " type-ahead 'o' highlighted " + s.active);
  await page.keyboard.press("Enter"); await wait(room, false); s = await state(room); out.picked = s;
  r.expect(s.text === "Room: Office" && s.native.room === "office" && s.focus && !s.popOpen, key + " Enter didn't pick, close and keep focus on the button: " + JSON.stringify({ text: s.text, native: s.native, focus: s.focus }));
  // Space opens; Escape closes and returns focus, with no change.
  await page.keyboard.press(" "); await wait(room, true);
  await page.keyboard.press("ArrowDown"); await page.keyboard.press("Escape"); await wait(room, false); s = await state(room);
  r.expect(s.text === "Room: Office" && s.focus && !s.popOpen, key + " Escape didn't close without a change and return focus: " + JSON.stringify({ text: s.text, focus: s.focus }));
  // A click outside closes it.
  await page.click(trig(room)); await wait(room, true); await page.click("#select-title"); await wait(room, false);
  s = await state(room); r.expect(!s.popOpen && s.text === "Room: Office", key + " a click outside didn't close it");
  // The 13-option list has a search field, focused on open; typing narrows the list and Enter picks the first match.
  await page.click(trig(device)); await wait(device, true); s = await state(device); out.device = s;
  r.expect(s.searchShown && s.searchFocus && s.options.length === 13 && inside(s.pop, s), key + " the 13-option list: " + JSON.stringify({ search: s.searchShown, focus: s.searchFocus, n: s.options.length, pop: s.pop }));
  await page.screenshot({ path: path.join(output, key + "-select-search.png"), fullPage: false });
  await page.keyboard.type("sens"); s = await state(device); r.expect(s.options.length === 1 && s.options[0].text === "Sensor" && s.active === "Sensor", key + " searching 'sens' left " + JSON.stringify(s.options.map((o) => o.text)));
  await page.keyboard.press("Control+a"); await page.keyboard.type("zzz"); s = await state(device); r.expect(s.options.length === 0 && s.empty, key + " no match doesn't say so");
  // "s" leaves All devices, Display, Sensor, Speaker and Thermostat; three Downs reach Speaker.
  await page.keyboard.press("Control+a"); await page.keyboard.type("s"); for (let i = 0; i < 3; i++) await page.keyboard.press("ArrowDown"); await page.keyboard.press("Enter"); await wait(device, false); s = await state(device);
  r.expect(s.native.device === "d11" && s.text === "Device: Speaker" && s.focus, key + " picking from a search: " + JSON.stringify({ text: s.text, native: s.native }));
  // Tab from the button, with the list open and no search field, picks the highlighted option and moves on to the next control.
  await page.click(trig(room)); await wait(room, true); await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Tab"); await wait(room, false); s = await state(room);
  const tabbed = await nextFocus(room); out.tabButton = { ...tabbed, text: s.text, native: s.native };
  r.expect(s.text === "Room: Garage" && s.native.room === "garage" && !s.popOpen && tabbed.onNext, key + " Tab from the button didn't pick, close and move on to the next control: " + JSON.stringify(out.tabButton));
  // A letter typed on the closed button (13 options: it has a search field) opens the list and lands in the field, not in a
  // type-ahead; more letters narrow it. The search field is a combobox that names the highlight.
  await page.focus(trig(device)); await page.keyboard.press("t");
  await wait(device, true); s = await state(device);
  const typed = await page.evaluate((root) => { const f = document.querySelector(root + " .sh-select-search input"); return { value: f.value, focus: document.activeElement === f, role: f.getAttribute("role"), expanded: f.getAttribute("aria-expanded"), controls: f.getAttribute("aria-controls"), listId: document.querySelector(root + ' [role="listbox"]').id, descendant: f.getAttribute("aria-activedescendant") }; }, device);
  out.typed = typed;
  r.expect(typed.value === "t" && typed.focus && s.options.length > 0 && s.options.every((o) => o.text.toLowerCase().includes("t")), key + " the first letter typed on the closed button didn't reach the search field: " + JSON.stringify({ typed, options: s.options.map((o) => o.text) }));
  r.expect(typed.role === "combobox" && typed.expanded === "true" && typed.controls === typed.listId && !!typed.descendant, key + " the search field isn't a combobox that controls the list and names the highlight: " + JSON.stringify(typed));
  await page.keyboard.type("h"); s = await state(device); r.expect(s.options.map((o) => o.text).join() === "Thermostat", key + " 'th' left " + JSON.stringify(s.options.map((o) => o.text)));
  await page.keyboard.press("Escape"); await wait(device, false);
  // Tab from the search field picks the highlighted option, closes the list and moves focus on to the next control after the button.
  await page.click(trig(device)); await wait(device, true); await page.keyboard.type("hea");
  await page.keyboard.press("Tab"); await wait(device, false); s = await state(device);
  const moved = await nextFocus(device);
  out.tab = { ...moved, text: s.text, native: s.native };
  r.expect(s.text === "Device: Heater" && s.native.device === "d3" && !s.popOpen && moved.onNext, key + " Tab from the search field didn't pick, close and move on to the next control: " + JSON.stringify(out.tab));
  // Room for the list is short below the button: it flips up, and stays inside the viewport.
  await page.evaluate((root) => document.querySelector(root).scrollIntoView({ block: "end" }), device);
  await page.click(trig(device)); await wait(device, true); s = await state(device); out.flip = s;
  r.expect(s.side === "top" && inside(s.pop, s) && s.pop.top >= s.barBottom - 0.5, key + " the list didn't flip up inside the viewport, under the top bar: " + JSON.stringify({ side: s.side, pop: s.pop, bar: s.barBottom, vh: s.vh }));
  await page.screenshot({ path: path.join(output, key + "-select-flip.png"), fullPage: false });
  await page.keyboard.press("Escape"); await wait(device, false);
  // With the button mid-page (about y=400 at 800 high) a 13-option list has more room above than below only by the top bar's
  // height: it stays under the bar (below the button, or above it but never higher than the bar's bottom edge).
  await page.evaluate((root) => document.querySelector(root).scrollIntoView({ block: "center" }), device);
  await page.click(trig(device)); await wait(device, true); s = await state(device); out.mid = s;
  r.expect(inside(s.pop, s) && s.pop.top >= s.barBottom - 0.5, key + " a mid-page list rose under the top bar or left the viewport: " + JSON.stringify({ side: s.side, pop: s.pop, bar: s.barBottom, vh: s.vh }));
  await page.keyboard.press("Escape"); await wait(device, false);
}

// The drawer's header row and first nav row, placed relative to the sidebar, with their type: what a shell page (whose
// header is shell::sidebar_head and whose rows are shell::NAV's markup) and the viewer must draw alike. Widths that
// follow the text (the brand name, a row's label) and, on desktop, the brand row's width (the viewer's collapse toggle
// sits beside it) are left out.
const drawerChrome = (page, mobile) => page.evaluate((mobile) => {
  const side = document.querySelector("#sidebar")?.getBoundingClientRect();
  const box = (element, origin = side) => {
    if (!element || !origin) return null;
    const b = element.getBoundingClientRect();
    return { left: b.left - origin.left, top: b.top - origin.top, width: b.width, height: b.height };
  };
  const type = (element) => {
    if (!element) return null;
    const style = getComputedStyle(element);
    return { size: style.fontSize, weight: style.fontWeight, line: style.lineHeight, family: style.fontFamily };
  };
  const pick = (object, keys) => object && Object.fromEntries(keys.map((key) => [key, object[key]]));
  const head = document.querySelector(".sidebar-head"), row = document.querySelector(".brandrow");
  const close = document.querySelector("#drawer-close"), item = document.querySelector("#nav .nav-item");
  const itemRect = item?.getBoundingClientRect();
  const label = item?.querySelector(":scope > span");
  return {
    head: mobile ? getComputedStyle(head ?? document.body).display : pick(box(head), ["top", "height"]),
    row: pick(box(row), mobile ? ["left", "top", "width", "height"] : ["left", "top", "height"]),
    rowType: type(row),
    mark: box(row?.querySelector(".mark")),
    name: pick(box(row?.querySelector(".brandname")), ["left", "top", "height"]),
    nameType: type(row?.querySelector(".brandname")),
    close: close && getComputedStyle(close).display !== "none" ? box(close) : "hidden",
    item: pick(box(item), ["left", "width", "height"]),
    itemType: type(item),
    icon: box(item?.querySelector(".icon"), itemRect),
    label: pick(box(label, itemRect), ["left", "top", "height"]),
    labelType: type(label),
  };
}, mobile);

// Every path where two drawerChrome readings differ: numbers by more than half a pixel, anything else at all.
function chromeDifferences(a, b, at = "") {
  if (typeof a === "number" && typeof b === "number") return Math.abs(a - b) <= 0.5 ? [] : [at + ": " + a + " vs " + b];
  if (a && b && typeof a === "object" && typeof b === "object") {
    return [...new Set([...Object.keys(a), ...Object.keys(b)])].flatMap((key) => chromeDifferences(a[key], b[key], at ? at + "." + key : key));
  }
  return a === b ? [] : [at + ": " + JSON.stringify(a) + " vs " + JSON.stringify(b)];
}

export default async function shellCheck(browser) {
  const r = reporter("shell");
  const results = {};
  const galleryHtml = fs.readFileSync(pages[0][1], "utf8");

  for (const [name, file] of pages) {
    const html = name === "shell-gallery" ? galleryHtml : fs.readFileSync(file, "utf8");
    for (const [widthName, width, height, mobile] of sizes) {
      for (const scheme of schemes) {
        const { context, page } = await galleryPage(browser, html, width, height, mobile, scheme);
        const key = name + "-" + widthName + "-" + scheme;
        results[key] = {};
        await page.screenshot({ path: path.join(output, key + ".png"), fullPage: true });

        const audit = await geometry(page);
        results[key].geometry = audit;
        if (name === "shell-gallery") {
          const longNameLength = await page.locator(".rows .row:last-child .sh-nm").evaluate((element) => element.textContent.length);
          results[key].longNameLength = longNameLength;
          r.expect(longNameLength === 90, key + " long row name length=" + longNameLength + " expected 90");
          // A link styled as a button reads as a button: no underline, and the button's own ink, not the link accent.
          const linkButton = await page.locator('a.btn[href="#learn-more"]').evaluate((element) => {
            const style = getComputedStyle(element), plain = getComputedStyle(document.querySelector(".btn-row button.btn:not(.primary)"));
            return { line: style.textDecorationLine, color: style.color, buttonColor: plain.color };
          });
          results[key].linkButton = linkButton;
          r.expect(linkButton.line === "none", key + " a.btn is underlined: " + JSON.stringify(linkButton));
          r.expect(linkButton.color === linkButton.buttonColor, key + " a.btn colour differs from a button's: " + JSON.stringify(linkButton));
          // The header is a direct child of the sidebar column here: it must not grow, so the nav sits right under it.
          if (!mobile) {
            const head = await page.evaluate(() => { const b = document.querySelector(".sidebar > .sidebar-head"), n = document.querySelector(".sidebar .nav-item"); if (!b || !n) return null; const x = b.getBoundingClientRect(), y = n.getBoundingClientRect(); return { brandTop: Math.round(x.top), brandBottom: Math.round(x.bottom), navTop: Math.round(y.top), gap: Math.round((y.top - x.bottom) * 10) / 10 }; });
            results[key].brandToNav = head;
            r.expect(!!head && head.gap >= 0 && head.gap <= 24 && head.brandBottom - head.brandTop <= 64, key + " the first nav item is not directly under the header: " + JSON.stringify(head));
          }
          // The drawer's header row and nav rows are the viewer's own, at both widths and in both schemes.
          const viewerPage = await served(browser, { size: mobile ? "phone" : "desktop", dark: scheme === "dark", path: "/" });
          await viewerPage.waitForSelector("#nav .nav-item", { state: "attached" });
          const [galleryDrawer, viewerDrawer] = await Promise.all([drawerChrome(page, mobile), drawerChrome(viewerPage, mobile)]);
          await viewerPage.context().close();
          const drawerDiff = chromeDifferences(galleryDrawer, viewerDrawer);
          results[key].drawerChrome = { gallery: galleryDrawer, viewer: viewerDrawer, differences: drawerDiff };
          r.expect(drawerDiff.length === 0, key + " the drawer's header or nav rows differ from the viewer's: " + drawerDiff.join("; "));
        }
        r.expect(audit.scrollWidth <= audit.innerWidth, key + " document scrollWidth=" + audit.scrollWidth + " innerWidth=" + audit.innerWidth);
        r.expect(audit.right.length === 0, key + " elements past the right edge: " + JSON.stringify(audit.right));
        if (mobile) {
          const targets = await phoneTargets(page);
          const shortTargets = targets.filter((target) => target.height < 44);
          results[key].phoneTargets = targets;
          r.expect(shortTargets.length === 0, key + " shell targets shorter than 44px: " + JSON.stringify(shortTargets));
          r.expect(audit.smallInputs.length === 0, key + " inputs below 16px: " + JSON.stringify(audit.smallInputs));
        }

        if (mobile && name === "shell-gallery") {
          const links = await page.evaluate(() => ["#paragraph-link", "#notice-link"].map((id) => {
            const element = document.querySelector(id);
            if (!element) return { id, missing: true };
            return { id, height: element.getBoundingClientRect().height, fontSize: Number.parseFloat(getComputedStyle(element).fontSize) };
          }));
          results[key].inlineLinks = links;
          r.expect(links.length === 2 && links.every((link) => !link.missing), key + " expected both inline links: " + JSON.stringify(links));
          for (const link of links) {
            if (!link.missing) r.expect(link.height <= link.fontSize * 1.6, key + " " + link.id + " is taller than inline text: " + JSON.stringify(link));
          }

          const viewerPage = await served(browser, { size: "phone", dark: scheme === "dark", path: "/" });
          await viewerPage.waitForSelector("#lead-btn");
          await viewerPage.waitForSelector(".nav-item", { state: "attached" });
          const [galleryChrome, viewerChrome] = await Promise.all([chromeDimensions(page), chromeDimensions(viewerPage)]);
          results[key].chromeDimensions = { gallery: galleryChrome, viewer: viewerChrome };
          for (const part of ["lead", "nav"]) {
            const gallerySize = galleryChrome[part], viewerSize = viewerChrome[part];
            r.expect(!!gallerySize && !!viewerSize && Math.abs(gallerySize.width - viewerSize.width) <= 0.01 && Math.abs(gallerySize.height - viewerSize.height) <= 0.01,
              key + " " + part + " dimensions differ from viewer: " + JSON.stringify({ gallery: gallerySize, viewer: viewerSize }));
          }
          await viewerPage.context().close();

          const line = await page.evaluate(() => {
            window.scrollTo(0, document.documentElement.scrollHeight);
            return new Promise((resolve) => requestAnimationFrame(() => resolve(document.querySelector("#topbar").classList.contains("scrolled"))));
          });
          results[key].scrolled = line;
          r.expect(line, key + " top bar did not gain .scrolled after scrolling");
          await page.evaluate(() => window.scrollTo(0, 0));
          await page.waitForTimeout(30);

          await page.click("#lead-btn");
          const openSidebarRect = await stableRect(page, "#sidebar");
          const drawerOpen = await page.evaluate(() => document.body.classList.contains("drawer-open"));
          const { scrimVisible, innerWidth: viewportWidth } = await page.evaluate(() => {
            const scrim = document.getElementById("scrim");
            const box = scrim?.getBoundingClientRect();
            const style = scrim ? getComputedStyle(scrim) : null;
            const scrimVisible = !!scrim && box.width > 0 && box.height > 0 && Number.parseFloat(style.opacity) > 0 && style.pointerEvents !== "none";
            return { scrimVisible, innerWidth };
          });
          results[key].drawerSidebar = { rect: openSidebarRect, scrimVisible };
          r.expect(!!openSidebarRect && openSidebarRect.left >= -0.5, key + " open drawer sidebar left=" + openSidebarRect?.left + " expected >= 0");
          r.expect(!!openSidebarRect && openSidebarRect.right <= viewportWidth + 0.5, key + " open drawer sidebar right=" + openSidebarRect?.right + " expected <= " + viewportWidth);
          r.expect(!!openSidebarRect && openSidebarRect.width >= 240, key + " open drawer sidebar width=" + openSidebarRect?.width + " expected >= 240");
          r.expect(scrimVisible, key + " scrim is not visible while the drawer is open");
          const drawerAudit = await geometry(page);
          const drawerTargets = await phoneTargets(page);
          const shortDrawerTargets = drawerTargets.filter((target) => target.height < 44);
          results[key].drawerGeometry = drawerAudit;
          r.expect(drawerAudit.scrollWidth <= drawerAudit.innerWidth, key + " drawer document scrollWidth=" + drawerAudit.scrollWidth + " innerWidth=" + drawerAudit.innerWidth);
          r.expect(drawerAudit.right.length === 0, key + " drawer elements past the right edge: " + JSON.stringify(drawerAudit.right));
          r.expect(shortDrawerTargets.length === 0, key + " drawer shell targets shorter than 44px: " + JSON.stringify(shortDrawerTargets));
          // A viewport screenshot, not fullPage: the drawer and scrim are fixed-position, so only the viewport shows
          // them where the user actually sees them.
          await page.screenshot({ path: path.join(output, key + "-drawer.png"), fullPage: false });
          // A popover opened from the drawer (an account menu, say): Esc closes it and leaves the drawer open.
          await page.evaluate(() => document.getElementById("drawer-popover").showPopover());
          await page.keyboard.press("Escape");
          const afterPopoverEscape = await page.evaluate(() => ({ popover: document.getElementById("drawer-popover").matches(":popover-open"), drawer: document.body.classList.contains("drawer-open") }));
          results[key].popoverEscape = afterPopoverEscape;
          r.expect(!afterPopoverEscape.popover && afterPopoverEscape.drawer, key + " Escape with a popover open should close only the popover: " + JSON.stringify(afterPopoverEscape));
          await page.keyboard.press("Escape");
          const closedSidebarRect = await stableRect(page, "#sidebar");
          const drawerAfterEscape = await page.evaluate(() => ({
            open: document.body.classList.contains("drawer-open"),
            focus: document.activeElement?.id,
            expanded: document.querySelector("#lead-btn")?.getAttribute("aria-expanded"),
          }));
          results[key].drawer = { drawerOpen, ...drawerAfterEscape, closedSidebarRect };
          r.expect(drawerOpen, key + " menu button did not open the drawer");
          r.expect(!drawerAfterEscape.open && drawerAfterEscape.focus === "lead-btn" && drawerAfterEscape.expanded === "false", key + " Escape did not close the drawer and restore focus: " + JSON.stringify(drawerAfterEscape));
          r.expect(!!closedSidebarRect && closedSidebarRect.right <= 0.5, key + " sidebar was not fully off screen after closing: " + JSON.stringify(closedSidebarRect));
          // Swiping the drawer shut with a popover open from it closes the popover too, so it doesn't float over the page.
          await page.click("#lead-btn"); await stableRect(page, "#sidebar");
          await page.evaluate(() => document.getElementById("drawer-popover").showPopover());
          await page.evaluate(() => {
            const side = document.getElementById("sidebar"), at = (x) => new Touch({ identifier: 1, target: side, clientX: x, clientY: 300 });
            side.dispatchEvent(new TouchEvent("touchstart", { touches: [at(250)], bubbles: true }));
            side.dispatchEvent(new TouchEvent("touchmove", { touches: [at(150)], bubbles: true }));
          });
          const afterSwipe = await page.evaluate(() => ({ popover: document.getElementById("drawer-popover").matches(":popover-open"), drawer: document.body.classList.contains("drawer-open") }));
          results[key].popoverSwipe = afterSwipe;
          r.expect(!afterSwipe.popover && !afterSwipe.drawer, key + " swiping the drawer shut should close its popover too: " + JSON.stringify(afterSwipe));
          await stableRect(page, "#sidebar");

          await page.click('#remove-form button[type="submit"]');
          await page.waitForFunction(() => document.querySelector("#remove-sheet")?.open === true);
          await stableRect(page, "#remove-sheet");
          const sheet = await page.evaluate(() => {
            const dialog = document.querySelector("#remove-sheet");
            const box = dialog.getBoundingClientRect();
            const main = document.getElementById("main");
            return { open: dialog.open, bottom: box.bottom, height: box.height, viewport: innerHeight, mainLeft: main ? main.getBoundingClientRect().left : null };
          });
          results[key].sheet = sheet;
          r.expect(sheet.open, key + " confirm sheet did not open");
          r.expect(Math.abs(sheet.bottom - sheet.viewport) <= 1, key + " confirm sheet bottom=" + sheet.bottom + " viewport=" + sheet.viewport);
          r.expect(sheet.mainLeft != null && Math.abs(sheet.mainLeft) <= 0.5, key + " main was shifted to left=" + sheet.mainLeft + " while the sheet was open (the page behind it must not move)");
          const openAudit = await geometry(page);
          const sheetTargets = await phoneTargets(page);
          const shortSheetTargets = sheetTargets.filter((target) => target.height < 44);
          r.expect(openAudit.scrollWidth <= openAudit.innerWidth, key + " sheet document scrollWidth=" + openAudit.scrollWidth + " innerWidth=" + openAudit.innerWidth);
          r.expect(openAudit.right.length === 0, key + " sheet elements past the right edge: " + JSON.stringify(openAudit.right));
          r.expect(shortSheetTargets.length === 0, key + " open sheet shell targets shorter than 44px: " + JSON.stringify(shortSheetTargets));
          // A viewport screenshot, not fullPage: dialog.sheet is fixed-position.
          await page.screenshot({ path: path.join(output, key + "-sheet.png"), fullPage: false });
          const clickPoints = await page.evaluate(() => {
            const box = document.querySelector("#remove-sheet").getBoundingClientRect();
            return { paddingX: box.left + 8, paddingY: box.top + box.height / 2, backdropX: innerWidth / 2, backdropY: box.top - 8 };
          });
          await page.mouse.click(clickPoints.paddingX, clickPoints.paddingY);
          const stayedOpen = await page.evaluate(() => document.querySelector("#remove-sheet").open);
          results[key].sheetPaddingClick = { points: clickPoints, stayedOpen };
          r.expect(stayedOpen, key + " click inside sheet padding closed the sheet");
          await page.mouse.click(clickPoints.backdropX, clickPoints.backdropY);
          const backdropClosed = await page.evaluate(() => !document.querySelector("#remove-sheet").open);
          results[key].sheetBackdropClick = { points: clickPoints, closed: backdropClosed };
          r.expect(backdropClosed, key + " click above the sheet did not close it");

          await page.click('button[data-open="details-sheet"]');
          const detailsOpen = await page.evaluate(() => document.querySelector("#details-sheet").open);
          r.expect(detailsOpen, key + " data-open did not open its sheet");
          const detailsTargets = await phoneTargets(page);
          const shortDetailsTargets = detailsTargets.filter((target) => target.height < 44);
          r.expect(shortDetailsTargets.length === 0, key + " details sheet targets shorter than 44px: " + JSON.stringify(shortDetailsTargets));
          await page.click('#details-sheet [data-close]');

          await page.click("[data-copy]");
          await page.waitForFunction(() => document.querySelector("[data-copy]").classList.contains("copied"));
          const copied = await page.evaluate(async () => ({
            value: await navigator.clipboard.readText(),
            label: document.querySelector("[data-copy]").getAttribute("aria-label"),
          }));
          results[key].copy = copied;
          r.expect(copied.value === "example-value-123", key + " copied text was " + JSON.stringify(copied.value));
          r.expect(copied.label === "Copied", key + " copy control did not show Copied feedback");
        }

        if (name === "shell-gallery") await selectChecks(page, key, mobile, r, results);

        results[key].pageErrors = page.errors;
        r.expect(page.errors.length === 0, key + " page errors: " + page.errors.join(" | "));
        await context.close();
      }
    }
  }

  r.results = results;
  return r.done();
}

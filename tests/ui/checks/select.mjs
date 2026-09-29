// The Analytics and Sessions filters are the shell's Select, not native selects (feat/select-component). The four Selects sit in the
// filters' sheet, which the Filter button opens (checks/filters.mjs covers the button, the chips and the sheet itself); here the
// sheet is opened first and closed with "Done" after the choice.
//
// On each page, at 390×844 and 1280×860 in light and dark:
//   - no <select> element is left on the page, and the four filters (Repo, Machine, Harness, Model) are comboboxes reading
//     "Label: value";
//   - opened, the Repo list shows "All repos" first and a check on the selected option, has a search field when it has more
//     than eight options (and none otherwise), and its text has AA contrast (4.5:1);
//   - at 1280 the list is a popover inside the viewport, under the button; at 390 it is a bottom sheet at the screen's bottom
//     edge whose rows are at least 44 px; neither leaves the page scrolling sideways;
//   - choosing "harbor" filters the page as the native select did (by keyboard at 1280: Enter opens, "h" jumps, Enter picks;
//     by taps at 390), and Escape closes the list and returns focus to the button.
// Screenshots: select-<page>-<size>-<scheme>-{closed,open}.png.
import path from "node:path";
import { ENV, served, reporter, overflow, contrastOf, filterSheet, doneFilterSheet } from "../lib.mjs";

const REPO = '.facet-filters .sh-select[data-label="Repo"]';

export default async function selectCheck(browser) {
  const r = reporter("select"), results = {};
  const state = (page) => page.evaluate((root) => {
    const box = document.querySelector(root), t = box.querySelector(".sh-select-trigger"), list = box.querySelector('[role="listbox"]'), pop = box.querySelector(".sh-select-pop"), sheet = box.querySelector("dialog.sh-select-sheet");
    const shown = [...list.querySelectorAll('[role="option"]')].filter((o) => o.getClientRects().length), rect = (n) => { const b = n?.getBoundingClientRect(); return b && b.width ? { left: b.left, top: b.top, right: b.right, bottom: b.bottom } : null; };
    const search = box.querySelector(".sh-select-search input"), trigger = t.getBoundingClientRect();
    return { text: t.textContent.trim(), role: t.getAttribute("role"), expanded: t.getAttribute("aria-expanded"), focus: document.activeElement === t, popOpen: !!pop && !pop.hidden, sheetOpen: !!sheet?.open,
      options: shown.map((o) => ({ text: o.textContent.trim(), selected: o.getAttribute("aria-selected"), check: getComputedStyle(o.querySelector(".sh-select-check")).visibility, height: o.getBoundingClientRect().height })),
      search: !!search && search.getClientRects().length > 0, pop: rect(pop), sheet: rect(sheet), triggerBottom: trigger.bottom, vw: document.documentElement.clientWidth, vh: innerHeight };
  }, REPO);
  const wait = (page, open) => page.waitForFunction(({ root, open }) => (document.querySelector(root + " .sh-select-trigger").getAttribute("aria-expanded") === "true") === open, { root: REPO, open });

  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const phone = size === "phone", scheme = dark ? "dark" : "light";
    for (const screen of ["analytics", "sessions"]) {
      const key = screen + "-" + (phone ? "390" : "1280") + "-" + scheme, rec = (results[key] = {});
      const page = await served(browser, { size, dark, path: "/" + screen });
      await page.waitForSelector(".facet-filters .sh-select-trigger", { state: "attached" });
      const built = await page.evaluate(() => ({ selects: document.querySelectorAll("#page select").length, triggers: [...document.querySelectorAll(".facet-filters .sh-select-trigger")].map((t) => t.textContent.trim()), roles: [...document.querySelectorAll(".facet-filters .sh-select-trigger")].map((t) => t.getAttribute("role")) }));
      rec.built = built;
      r.expect(built.selects === 0, key + " native selects left on the page: " + built.selects);
      r.expect(built.triggers.join("|") === "Repo: All repos|Machine: All machines|Harness: All harnesses|Model: All models" && built.roles.every((x) => x === "combobox"), key + " the filters: " + JSON.stringify(built));
      // A value the ellipsis cuts off is shown in full by a tooltip (data-tip, only while clipped).
      rec.tips = await page.evaluate(() => [...document.querySelectorAll(".facet-filters .sh-select-trigger")].map((t) => [t.textContent.trim(), t.querySelector(".sh-select-text")?.dataset.tip, t.querySelector(".sh-select-text")?.hasAttribute("data-tip-clipped")]));
      r.expect(rec.tips.length === 4 && rec.tips.every(([text, tip, clipped]) => tip === text && clipped), key + " the trigger's tooltip isn't its full text, shown only while clipped: " + JSON.stringify(rec.tips));
      rec.overflowClosed = await overflow(page); r.expect(rec.overflowClosed === 0, key + " sideways overflow, closed: " + rec.overflowClosed);
      await page.locator(".facet-filters").scrollIntoViewIfNeeded();
      await page.screenshot({ path: path.join(ENV.out, "select-" + key + "-closed.png"), fullPage: false });
      // The filter bar is tagged: a navigation (the viewer routing on a history pop) would draw a new one.
      const kept = () => page.evaluate(() => document.querySelector(".facet-filters")?.__kept === true);
      await page.evaluate(() => { document.querySelector(".facet-filters").__kept = true; });
      // A phone's page is scrolled first, so the scroll lock has a baseline to hold.
      const scrolledTo = phone ? await page.evaluate(() => { scrollTo(0, 40); return scrollY; }) : 0;
      if (phone) r.expect(scrolledTo > 0, key + " the page isn't scrolled before the sheet opens: " + scrolledTo);
      if (phone) {
        // The probe's control: with no sheet open a wheel does scroll the page.
        await page.mouse.move(195, 400); await page.mouse.wheel(0, 200); await page.waitForTimeout(200);
        const moved = await page.evaluate(() => scrollY); rec.wheelControl = { from: scrolledTo, to: moved };
        r.expect(moved > scrolledTo, key + " a wheel doesn't scroll the page even with no sheet open, so the lock check proves nothing: " + JSON.stringify(rec.wheelControl));
        await page.evaluate((y) => scrollTo(0, y), scrolledTo);
      }

      // Open the sheet, then the Repo list. The sheet doesn't move the page either.
      await filterSheet(page);
      const sheetY = await page.evaluate(() => scrollY); rec.sheetY = { before: scrolledTo, open: sheetY };
      r.expect(sheetY === scrolledTo, key + " opening the filters' sheet moved the page: " + JSON.stringify(rec.sheetY));
      await page.click(REPO + " .sh-select-trigger"); await wait(page, true);
      let s = await state(page); rec.open = s;
      const count = s.options.length;
      r.expect(count >= 2 && s.options[0].text === "All repos", key + " the list doesn't start with All repos: " + JSON.stringify(s.options.map((o) => o.text)));
      r.expect(s.options.filter((o) => o.selected === "true").length === 1 && s.options[0].selected === "true" && s.options.filter((o) => o.check === "visible").length === 1 && s.options[0].check === "visible", key + " the selected option isn't the only one with a check: " + JSON.stringify(s.options));
      r.expect(s.search === (count > 8), key + " search field " + s.search + " for " + count + " options");
      rec.overflowOpen = await overflow(page); r.expect(rec.overflowOpen === 0, key + " sideways overflow, open: " + rec.overflowOpen);
      const inside = (b) => !!b && b.left >= -0.5 && b.top >= -0.5 && b.right <= s.vw + 0.5 && b.bottom <= s.vh + 0.5;
      if (phone) {
        r.expect(s.sheetOpen && !s.popOpen && !!s.sheet && Math.abs(s.sheet.bottom - s.vh) <= 1 && s.sheet.left <= 0.5 && s.sheet.right >= s.vw - 0.5, key + " a phone didn't open a full-width sheet at the bottom edge: " + JSON.stringify(s.sheet));
        r.expect(s.options.every((o) => o.height >= 44), key + " sheet rows under 44 px: " + JSON.stringify(s.options.map((o) => o.height)));
      } else {
        r.expect(s.popOpen && !s.sheetOpen && inside(s.pop) && s.pop.top >= s.triggerBottom - 1, key + " the popover isn't open inside the viewport under the button: " + JSON.stringify({ pop: s.pop, triggerBottom: s.triggerBottom }));
      }
      rec.contrast = {};
      for (const [name, selector] of [["label", REPO + " .sh-select-label"], ["value", REPO + " .sh-select-value"], ["option", REPO + " .sh-select-option"], ["active", REPO + " .sh-select-option.sh-active"]]) {
        const ratio = await contrastOf(page, selector); rec.contrast[name] = ratio;
        r.expect(ratio != null && ratio >= 4.5, key + " " + name + " text contrast " + ratio + " is under 4.5");
      }
      await page.screenshot({ path: path.join(ENV.out, "select-" + key + "-open.png"), fullPage: false });

      if (phone) {
        // The page behind the sheet doesn't scroll, and Back closes the sheet without leaving the page.
        const lock = await page.evaluate(() => ({ overflow: getComputedStyle(document.documentElement).overflow, y: scrollY, entry: history.state?.shSelect ?? null, route: history.state?.v ?? null, path: location.pathname }));
        await page.mouse.move(s.vw / 2, 20); await page.mouse.wheel(0, 400); await page.waitForTimeout(200);
        const y = await page.evaluate(() => scrollY);
        rec.lock = { ...lock, after: y };
        r.expect(lock.y === scrolledTo && y === scrolledTo, key + " opening the sheet moved the page, or a wheel over its backdrop scrolled it: " + JSON.stringify(rec.lock));
        r.expect(lock.entry != null, key + " opening the sheet pushed no history entry: " + JSON.stringify(lock));
        await page.goBack(); await wait(page, false);
        const back = await page.evaluate(() => ({ overflow: getComputedStyle(document.documentElement).overflow, entry: history.state?.shSelect ?? null, path: location.pathname, filters: document.querySelectorAll(".facet-filters .sh-select-trigger").length }));
        s = await state(page); rec.back = { ...back, sheet: s.sheetOpen };
        rec.back.sameBar = await kept();
        r.expect(!s.sheetOpen && back.path === lock.path && back.entry == null && back.filters === 4 && rec.back.sameBar, key + " Back didn't close the sheet and stay on the page (the viewer re-routed if the filter bar is new): " + JSON.stringify(rec.back));
        await page.click(REPO + " .sh-select-trigger"); await wait(page, true);
      }
      // Escape closes and returns focus.
      await page.keyboard.press("Escape"); await wait(page, false); s = await state(page);
      rec.escapeSameBar = await kept();
      r.expect(rec.escapeSameBar && !s.popOpen && !s.sheetOpen && s.focus && s.text === "Repo: All repos", key + " Escape didn't close the list, keep the choice and return focus: " + JSON.stringify({ text: s.text, focus: s.focus }));

      // Choose harbor: the page filters as the native select did, once the sheet is closed.
      const before = await page.evaluate((screen) => screen === "sessions" ? document.querySelectorAll("#page .nrow").length : document.querySelectorAll('[data-breakdown="repo"]').length, screen);
      if (phone) {
        await page.click(REPO + " .sh-select-trigger"); await wait(page, true);
        await page.locator(REPO + " .sh-select-option", { hasText: "harbor" }).click();
      } else {
        await page.focus(REPO + " .sh-select-trigger"); await page.keyboard.press("Enter"); await wait(page, true);
        await page.keyboard.press("h"); await page.keyboard.press("Enter");
      }
      await wait(page, false);
      await page.waitForFunction(() => document.querySelector('.facet-filters .sh-select[data-label="Repo"] .sh-select-trigger').textContent.trim() === "Repo: harbor");
      s = await state(page); rec.chosenState = { text: s.text, focus: s.focus };
      await doneFilterSheet(page);
      // Analytics is answered by the server for the range and filters: wait until the figures drawn are for this repo.
      if (screen === "analytics") await page.waitForFunction(() => /(^|&)repo=harbor(&|$)/.test(document.querySelector(".analytics-metrics[data-analytics-ready]")?.dataset.query ?? ""));
      const after = await page.evaluate((screen) => screen === "sessions"
        ? { rows: document.querySelectorAll("#page .nrow").length, other: [...document.querySelectorAll("#page .nrow .for")].filter((f) => !f.textContent.includes("harbor")).length }
        : { rows: document.querySelectorAll('[data-breakdown="repo"]').length, other: [...document.querySelectorAll('[data-breakdown="repo"]')].filter((b) => b.dataset.key !== "harbor").length }, screen);
      rec.chosen = { before, after, text: rec.chosenState.text, focus: rec.chosenState.focus, sameBar: await kept(), path: await page.evaluate(() => location.pathname) };
      r.expect(rec.chosen.text === "Repo: harbor" && rec.chosen.focus && after.rows > 0 && after.rows < before && after.other === 0, key + " choosing harbor: " + JSON.stringify(rec.chosen));
      r.expect(rec.chosen.sameBar && rec.chosen.path === "/" + screen, key + " closing the list by hand re-routed the viewer: " + JSON.stringify(rec.chosen));
      if (screen === "analytics") r.expect(after.rows === 1, key + " Analytics still breaks down " + after.rows + " repos after filtering to one");
      r.expect(page.errors.length === 0, key + " page errors: " + page.errors.join(" | "));
      await page.context().close();
    }
  }
  r.results = results;
  return r.done();
}

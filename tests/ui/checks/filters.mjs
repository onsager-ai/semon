// The filters of Analytics and Sessions: one Filter button, one chip per active filter, and a sheet (fix/filters-sheet).
//
// On each page, at 390×844 and 1280×860 in light and dark:
//   - with nothing active the control is one line under 56 px (the Filter button alone: no chip, no count), its text is --fs-sm, it
//     is a 44 px tap target on a phone, and no <select> is left on the page;
//   - at 390 the first figure (Analytics) or first session row (Sessions) starts higher than it did with the four selects. The
//     four selects of main are rebuilt in place from main's own rules (a 2-column grid of --tap tall selects at 390, 4 columns at
//     1280, gap 8, padding 10px 0, a 1px border below), so the comparison is made in the same run and needs no constant;
//   - the sheet opens from the button (a bottom sheet at 390, a dialog inside the viewport at 1280), its first row starts below its
//     header, Tab never leaves it, every control has a name, Escape closes it and returns focus to the Filter button, and at 390
//     Back closes it without leaving the page;
//   - choosing filters in the sheet shows a chip for each ("Repo: harbor ×") and the count on the button; a chip's × clears exactly
//     that filter (the other stays, and the page shows what the other filter alone showed); "Clear all" clears every one;
//   - the page never scrolls sideways, closed, open, or with chips.
// Screenshots: filters-<page>-<size>-<scheme>-{closed,chips,open}.png.
import path from "node:path";
import { ENV, served, reporter, overflow, filterSheet, doneFilterSheet, pickFilter, FILTER_BUTTON, FILTER_SHEET } from "../lib.mjs";

const FIRST = { analytics: ".analytics-metric", sessions: "#page .nrow" };
// What each page shows, to compare a filter's outcome with another's.
const shown = (page, screen) => page.evaluate((screen) => screen === "sessions"
  ? [...document.querySelectorAll("#page .nrow")].map((x) => x.dataset.id).sort().join(",")
  : [...document.querySelectorAll('[data-breakdown="repo"]')].map((x) => x.dataset.key).sort().join(","), screen);
const drawn = (page, screen, query) => screen === "analytics" ? page.waitForFunction((q) => { const m = document.querySelector(".analytics-metrics[data-analytics-ready]"); return !!m && m.dataset.query === q; }, query) : page.waitForTimeout(150);
const backgroundScroll = (page) => page.evaluate(() => matchMedia("(max-width: 760px)").matches ? scrollY : document.querySelector("#main").scrollTop);
const facetTexts = (page) => page.evaluate(() => [...document.querySelectorAll(".facet-filters .sh-select-trigger")].map((t) => t.textContent.trim()));
const chipTexts = (page) => page.evaluate(() => [...document.querySelectorAll(".facet-filters .facet-chip")].filter((c) => c.getClientRects().length).map((c) => c.textContent.trim()));

export default async function filtersCheck(browser) {
  const r = reporter("filters"), results = {};
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const phone = size === "phone", scheme = dark ? "dark" : "light";
    for (const screen of ["analytics", "sessions"]) {
      const key = screen + "-" + (phone ? "390" : "1280") + "-" + scheme, rec = (results[key] = {});
      const shot = (name) => page.screenshot({ path: path.join(ENV.out, "filters-" + key + "-" + name + ".png"), fullPage: false });
      const page = await served(browser, { size, dark, path: "/" + screen });
      await page.waitForSelector(FILTER_BUTTON);
      if (screen === "analytics") await drawn(page, screen, "range=7d");

      // ---- Nothing active: one line ----
      rec.closed = await page.evaluate(({ first }) => {
        const bar = document.querySelector(".facet-filters"), btn = bar.querySelector(".facet-btn"), b = bar.getBoundingClientRect(), t = btn.getBoundingClientRect(), n = bar.querySelector(".facet-n");
        const visible = [...bar.children].filter((c) => c.getClientRects().length);
        return { height: b.height, button: t.height, buttonWidth: t.width, font: getComputedStyle(btn).fontSize, name: btn.getAttribute("aria-label"), text: btn.textContent.trim(), tops: [...new Set(visible.map((c) => Math.round(c.getBoundingClientRect().top)))].length, visible: visible.map((c) => c.className), count: !!n && n.getClientRects().length > 0, chips: bar.querySelectorAll(".facet-chip").length && [...bar.querySelectorAll(".facet-chip")].filter((c) => c.getClientRects().length).length,
          selects: document.querySelectorAll("#page select").length, firstTop: document.querySelector(first).getBoundingClientRect().top + scrollY, barTop: b.top + scrollY, barBottom: b.bottom + scrollY, ph: getComputedStyle(document.documentElement).getPropertyValue("--fs-sm") };
      }, { first: FIRST[screen] });
      const c = rec.closed;
      r.expect(c.height < 56 && c.tops === 1 && c.visible.join() === "facet-btn" && !c.count && c.chips === 0, key + " with nothing active the control isn't one line under 56 px: " + JSON.stringify(c));
      r.expect(c.font === "13px" && c.text === "Filter" && c.name === "Filter" && (!phone || c.button >= 44), key + " the Filter button isn't --fs-sm, named Filter, and 44 px on a phone: " + JSON.stringify(c));
      r.expect(c.selects === 0, key + " native selects left on the page: " + c.selects);
      // main's four selects, put back where the control is, for the same run's comparison.
      rec.main = await page.evaluate(({ first, phone }) => {
        const bar = document.querySelector(".facet-filters"), probe = document.createElement("div");
        probe.style.cssText = "display:grid;grid-template-columns:repeat(" + (phone ? 2 : 4) + ",minmax(0,1fr));gap:8px;padding:10px 0;border-bottom:1px solid var(--line)";
        for (let i = 0; i < 4; i++) { const s = document.createElement("div"); s.style.height = "var(--tap)"; probe.append(s); }
        bar.style.display = "none"; bar.after(probe);
        const top = document.querySelector(first).getBoundingClientRect().top + scrollY, height = probe.getBoundingClientRect().height;
        probe.remove(); bar.style.display = ""; return { firstTop: top, control: height };
      }, { first: FIRST[screen], phone });
      rec.saved = rec.main.firstTop - c.firstTop;
      r.expect(rec.saved > 0, key + " the first " + (screen === "analytics" ? "figure" : "session row") + " isn't higher than with the four selects: " + JSON.stringify({ closed: c.firstTop, main: rec.main.firstTop }));
      if (phone) r.expect(rec.saved >= rec.main.control - 56, key + " the first content moved up by " + rec.saved + " px, under the control's own saving of " + (rec.main.control - 56) + ": " + JSON.stringify({ closed: c.firstTop, main: rec.main.firstTop }));
      rec.overflowClosed = await overflow(page); r.expect(rec.overflowClosed === 0, key + " sideways overflow, closed: " + rec.overflowClosed);
      await page.locator(".facet-filters").scrollIntoViewIfNeeded();
      await shot("closed");

      // ---- The sheet ----
      // A phone's page is scrolled first: opening the sheet must not send it back to the top.
      const y0 = await page.evaluate(() => {
        // Supply a scroll range even when desktop content fits in the viewport.
        const probe = document.createElement("div"); probe.id = "filter-scroll-probe";
        const phone = matchMedia("(max-width: 760px)").matches, scroller = phone ? document.body : document.querySelector("#main");
        probe.style.cssText = "position:absolute;pointer-events:none;width:1px;height:500px;top:" + Math.max(scroller.scrollHeight, scroller.clientHeight) + "px";
        scroller.append(probe); if (phone) scrollTo(0, 40); else scroller.scrollTop = 0; return phone ? scrollY : scroller.scrollTop;
      });
      if (phone) r.expect(y0 > 0, key + " the page isn't scrolled before the sheet opens: " + y0);
      const wheelPoint = { x: (await page.evaluate(() => innerWidth)) - 8, y: 5 };
      await page.mouse.move(wheelPoint.x, wheelPoint.y); await page.mouse.wheel(0, 240); await page.waitForTimeout(200);
      rec.wheelControl = await backgroundScroll(page);
      r.expect(rec.wheelControl > y0, key + " closed-page wheel control didn't scroll: " + JSON.stringify({ before: y0, after: rec.wheelControl }));
      await page.evaluate((y) => { if (matchMedia("(max-width: 760px)").matches) scrollTo(0, y); else document.querySelector("#main").scrollTop = y; }, y0);
      await filterSheet(page);
      rec.sheet = await page.evaluate(({ sheetSel }) => {
        const d = document.querySelector(sheetSel), r = (n) => { const b = n.getBoundingClientRect(); return { left: b.left, top: b.top, right: b.right, bottom: b.bottom, width: b.width, height: b.height }; };
        const head = d.querySelector(".vh"), first = d.querySelector(".sh-select"), body = d.querySelector(".vb");
        const unnamed = [...d.querySelectorAll("button, [role=combobox], input, select")].filter((x) => x.getClientRects().length).filter((x) => { const ids = (x.getAttribute("aria-labelledby") ?? "").split(/\s+/).filter(Boolean).map((i) => document.getElementById(i)?.textContent.trim()).join(""); return !(x.getAttribute("aria-label") || ids || x.textContent.trim()); }).map((x) => x.className);
        return { dialog: r(d), head: r(head), first: r(first), scrollTop: body.scrollTop, name: d.getAttribute("aria-label"), modal: d.matches(":modal"), y: scrollY, vw: document.documentElement.clientWidth, vh: innerHeight, unnamed,
          buttons: [...d.querySelectorAll(".vf button")].map((x) => x.textContent.trim()), triggers: [...d.querySelectorAll(".sh-select-trigger")].map((x) => x.textContent.trim()), focus: d.contains(document.activeElement), active: document.activeElement?.className ?? null };
      }, { sheetSel: FILTER_SHEET });
      const sh = rec.sheet;
      r.expect(sh.modal && sh.name === "Filter" && sh.buttons.join("|") === "Clear all|Done" && sh.triggers.join("|") === "Repo: All repos|Machine: All machines|Harness: All harnesses|Model: All models", key + " the sheet isn't a modal Filter dialog with the four choices, Clear all and Done: " + JSON.stringify(sh));
      r.expect(sh.first.top >= sh.head.bottom - 0.5 && sh.scrollTop === 0, key + " the sheet's first row is under its header: " + JSON.stringify({ head: sh.head, first: sh.first }));
      r.expect(sh.unnamed.length === 0, key + " a control in the sheet has no label: " + JSON.stringify(sh.unnamed));
      r.expect(sh.y === y0, key + " opening the sheet moved the page: " + JSON.stringify({ before: y0, open: sh.y }));
      r.expect(sh.focus, key + " focus didn't move into the sheet: " + JSON.stringify(sh.active));
      if (phone) r.expect(Math.abs(sh.dialog.bottom - sh.vh) <= 1 && sh.dialog.left <= 0.5 && sh.dialog.right >= sh.vw - 0.5 && sh.dialog.top > 0, key + " the sheet isn't a bottom sheet: " + JSON.stringify(sh.dialog));
      else r.expect(sh.dialog.left >= 0 && sh.dialog.right <= sh.vw && sh.dialog.top >= 0 && sh.dialog.bottom <= sh.vh && sh.dialog.width <= 440, key + " the sheet isn't a dialog inside the viewport: " + JSON.stringify(sh.dialog));
      rec.overflowOpen = await overflow(page); r.expect(rec.overflowOpen === 0, key + " sideways overflow, open: " + rec.overflowOpen);
      await shot("open");
      // Exercise the input hold itself, rather than only checking the position
      // after opening. Cancelable moves outside the sheet must be refused.
      rec.held = await page.evaluate(() => ["wheel", "touchmove"].map((type) => {
        const event = type === "wheel" ? new WheelEvent(type, { bubbles: true, cancelable: true, deltaY: 240 })
          : new TouchEvent(type, { bubbles: true, cancelable: true });
        document.body.dispatchEvent(event);
        return { type, prevented: event.defaultPrevented };
      }));
      r.expect(rec.held.every((move) => move.prevented), key + " background input wasn't held: " + JSON.stringify(rec.held));
      await page.mouse.move(wheelPoint.x, wheelPoint.y); await page.mouse.wheel(0, 240); await page.waitForTimeout(200);
      rec.afterWheel = await backgroundScroll(page);
      r.expect(rec.afterWheel === y0, key + " wheel moved the page behind the sheet: " + JSON.stringify({ before: y0, after: rec.afterWheel }));
      // Tab stays inside the sheet (both ways), and Escape closes it and gives the focus back to the button.
      const stray = [];
      for (let i = 0; i < 12; i++) { await page.keyboard.press(i % 4 === 3 ? "Shift+Tab" : "Tab"); if (!(await page.evaluate((sel) => { const d = document.querySelector(sel); return document.activeElement !== document.body && d.contains(document.activeElement); }, FILTER_SHEET))) stray.push(i); }
      rec.stray = stray; r.expect(stray.length === 0, key + " Tab left the sheet at step(s) " + stray.join());
      await page.keyboard.press("Escape"); await page.waitForFunction((sel) => document.querySelector(sel).open === false, FILTER_SHEET);
      rec.afterEscape = await page.evaluate((btn) => ({ focus: document.activeElement === document.querySelector(btn), y: scrollY }), FILTER_BUTTON);
      r.expect(rec.afterEscape.focus && rec.afterEscape.y === y0, key + " Escape didn't return focus to the Filter button, or moved the page: " + JSON.stringify(rec.afterEscape));
      rec.released = await page.evaluate(() => ["wheel", "touchmove"].map((type) => {
        const event = new Event(type, { bubbles: true, cancelable: true });
        document.body.dispatchEvent(event);
        return { type, prevented: event.defaultPrevented };
      }));
      r.expect(rec.released.every((move) => !move.prevented), key + " closing the sheet left an input hold: " + JSON.stringify(rec.released));
      await page.mouse.move(wheelPoint.x, wheelPoint.y); await page.mouse.wheel(0, 240); await page.waitForTimeout(200);
      rec.wheelReleased = await backgroundScroll(page);
      r.expect(rec.wheelReleased > y0, key + " wheel remained blocked after Escape: " + JSON.stringify({ before: y0, after: rec.wheelReleased }));
      await page.evaluate(() => { scrollTo(0, 0); document.querySelector("#main").scrollTop = 0; document.getElementById("filter-scroll-probe").remove(); });
      if (phone) {
        // Back closes the sheet; the page stays.
        const path0 = await page.evaluate(() => location.pathname);
        await filterSheet(page); await page.goBack(); await page.waitForFunction((sel) => document.querySelector(sel).open === false, FILTER_SHEET);
        rec.back = { path: await page.evaluate(() => location.pathname), same: path0 };
        r.expect(rec.back.path === path0 && await page.locator(FILTER_BUTTON).count() === 1, key + " Back didn't close the sheet and stay on the page: " + JSON.stringify(rec.back));
      }

      // ---- Chips ----
      await pickFilter(page, "Harness", "claude");
      await drawn(page, screen, "range=7d&harness=claude");
      const harnessOnly = await shown(page, screen);
      await pickFilter(page, "Repo", "harbor");
      await drawn(page, screen, "range=7d&repo=harbor&harness=claude");
      const both = await shown(page, screen);
      rec.chips = { texts: await chipTexts(page), name: await page.getAttribute(FILTER_BUTTON, "aria-label"), badge: await page.locator(FILTER_BUTTON + " .facet-n").textContent(), harnessOnly, both };
      r.expect(rec.chips.texts.join("|") === "Repo: harbor|Harness: Claude Code" && rec.chips.badge === "2" && rec.chips.name === "Filter, 2 active", key + " the chips or the count for two filters: " + JSON.stringify(rec.chips));
      r.expect(both !== harnessOnly && both.length > 0, key + " the repo filter changed nothing on top of the harness filter: " + JSON.stringify({ harnessOnly, both }));
      rec.overflowChips = await overflow(page); r.expect(rec.overflowChips === 0, key + " sideways overflow with two chips: " + rec.overflowChips);
      rec.chipBox = await page.evaluate(() => [...document.querySelectorAll(".facet-filters .facet-chip")].filter((c) => c.getClientRects().length).map((c) => { const b = c.getBoundingClientRect(); return { h: b.height, w: b.width, hit: getComputedStyle(c, "::before").content === "none" ? b.height : b.height + 8 }; }));
      r.expect(rec.chipBox.every((b) => !phone || b.hit >= 44), key + " a chip's tap target is under 44 px on a phone: " + JSON.stringify(rec.chipBox));
      await page.locator(".facet-filters").scrollIntoViewIfNeeded();
      await shot("chips");
      // × on the Repo chip clears the repo and nothing else.
      await page.click('.facet-filters .facet-chip[data-facet="repo"]');
      await drawn(page, screen, "range=7d&harness=claude");
      rec.cleared = { texts: await chipTexts(page), triggers: await facetTexts(page), shown: await shown(page, screen), name: await page.getAttribute(FILTER_BUTTON, "aria-label"), focus: await page.evaluate((btn) => document.activeElement === document.querySelector(btn), FILTER_BUTTON) };
      r.expect(rec.cleared.texts.join("|") === "Harness: Claude Code" && rec.cleared.triggers[0] === "Repo: All repos" && rec.cleared.triggers[2] === "Harness: Claude Code" && rec.cleared.name === "Filter, 1 active", key + " the Repo chip's × didn't clear exactly the repo filter: " + JSON.stringify(rec.cleared));
      r.expect(rec.cleared.shown === harnessOnly, key + " with the repo cleared the page doesn't show what the harness filter alone showed: " + JSON.stringify({ harnessOnly, now: rec.cleared.shown }));
      r.expect(rec.cleared.focus, key + " the focus didn't go to the Filter button after a chip was cleared");
      // Clear all clears every filter and closes the sheet.
      await pickFilter(page, "Repo", "harbor"); await drawn(page, screen, "range=7d&repo=harbor&harness=claude");
      await filterSheet(page); await page.click(FILTER_SHEET + " .fclear"); await page.waitForFunction((sel) => document.querySelector(sel).open === false, FILTER_SHEET);
      await drawn(page, screen, "range=7d");
      rec.all = { texts: await chipTexts(page), triggers: await facetTexts(page), name: await page.getAttribute(FILTER_BUTTON, "aria-label"), height: await page.evaluate(() => document.querySelector(".facet-filters").getBoundingClientRect().height) };
      r.expect(rec.all.texts.length === 0 && rec.all.triggers.join("|") === "Repo: All repos|Machine: All machines|Harness: All harnesses|Model: All models" && rec.all.name === "Filter" && rec.all.height < 56, key + " Clear all didn't clear every filter and leave one line: " + JSON.stringify(rec.all));
      r.expect(page.errors.length === 0, key + " page errors: " + page.errors.join(" | "));
      await page.context().close();
    }
  }
  r.results = results;
  return r.done();
}

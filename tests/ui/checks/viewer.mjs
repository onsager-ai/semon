// Ported from the mockup's viewer.js. The original had two independent halves: a phone-dark half that drove
// test-real.html (real logs) — a census of every step's cut/uncut preview against whether "View all" shows, then
// opened the longest cut call's "View all" sheet and checked its geometry, its full text, and that back/close/Escape
// each restore the page exactly — and a desktop-light half that already drove test7.html (the sample mockup) to
// check the same dialog on a wide screen and that a backdrop click closes it.
//
// Ported: the phone half now drives the served sample fixture instead of test-real.html (the same measurements, on
// the fixture's data); the desktop half already used the sample, so it only changes how the page is reached. Session
// ids are unchanged from the sample. Neither half named a sample handoff or turn id, so there is no id mapping here.
// The sidebar now shows eight top-level sessions and nested children, so the census visits each served session by
// route instead of treating every tree row as a visible, flat sidebar item. The phone half opens whichever session
// the census found a cut preview on (discovered at runtime, never hardcoded to "harbor"); the desktop half opens
// 'harbor' by session id, which is stable and does have a cut preview on its first step.
//
// Tool entries carry `src` and may carry `more` in the served viewer: "View all" shows whenever the server cut a
// preview, even where the preview isn't visually clipped, and the sheet fetches the full text from /api/entry before
// it opens (see gaps.json: previews are short in this fixture except harbor's first Bash output).
//
// Assertions:
//  - no page errors, phone or desktop.
//  - the phone census: mismatch === 0 (every cut preview shows "View all" and vice versa, for every step on every lane).
//  - the "View all" sheet: it opens (open === true), sits inside the viewport (sideways === 0), and its fetched full
//    text for every <pre> is at least as long as what the (possibly cut) preview showed — i.e. "View all" actually
//    fetched more, never less.
//  - back, the close button and Escape each close the sheet, restore the same history state, and leave the page's
//    expanded steps as they were (afterBack/afterClose/afterEsc: dialog === false, sameState === true, stepsStillOpen
//    unchanged, and back also drops the viewer-open html class).
//  - the measure at 1280, light and dark: an assistant message is at most 68ch wide (in its own font) with wide mode off, and
//    with it on is wider than 68ch and fills its column.
//  - the desktop dialog: at least one "View all" is visible on the sample's first expanded step, the dialog is not
//    sideways-clipped off the 1280px viewport, and a backdrop click closes it (closedByBackdrop === true).
import path from "node:path";
import { ENV, served, data, goto, reporter, wide } from "../lib.mjs";

export default async function viewerCheck(browser) {
  const D = await data();
  const r = reporter("viewer");
  const R = {};

  // ---- Phone, dark: census on the sample lanes, then the "View all" sheet on the extras fixture's long output. ----
  {
    const page = await served(browser, { size: "phone", dark: true, extras: true });
    const lanes = Object.keys(D.SESS);
    const C = { steps: 0, cut: 0, viewAllShown: 0, mismatch: 0 }; const cutLanes = [];
    for (const id of lanes) {
      await goto(page, { v: "session", id }, D); await page.waitForTimeout(150);
      await page.evaluate(() => { for (let k = 0; k < 3; k++) document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click());
        document.querySelectorAll('.step > button[aria-expanded="false"]').forEach((x) => x.click()); });
      // ResizeObserver measures previews after their containing turn opens; inspect the resulting state on a later frame.
      await page.waitForTimeout(100);
      const rr = await page.evaluate(() => { const r = { steps: 0, cut: 0, viewAllShown: 0, mismatch: 0, bad: [] };
        document.querySelectorAll('.step > button[aria-expanded="true"]').forEach((x) => { r.steps++; const o = x.parentElement.querySelector('.out');
          const cut = /^Output\s*·\s*(first|last)\s+\d+/.test([...o.querySelectorAll('.io')].at(-1)?.textContent ?? "") || /^Cut short/.test(o.querySelector('.cutnote')?.textContent ?? "") || o.querySelector('.viewall:not(.viewscript) span')?.textContent === "View all"; /* a bare "View all", with no line count, is offered where the server cut the preview */ const shown = !!o.querySelector('.viewall:not(.viewscript)');
          if (cut) r.cut++; if (shown) r.viewAllShown++; if (cut !== shown) { r.mismatch++; r.bad.push({ arg: x.querySelector('.sa')?.textContent.slice(0, 60), cut, shown, ios: [...o.querySelectorAll('.io')].map((i) => i.textContent), all: o.querySelector('.viewall:not(.viewscript)')?.textContent ?? null, lines: [...o.querySelectorAll('pre')].map((p) => p.textContent.split('\n').length) }); } }); return r; });
      for (const k of ["steps", "cut", "viewAllShown", "mismatch"]) C[k] += rr[k];
      (C.bad ??= []).push(...rr.bad);
      if (rr.cut) cutLanes.push(id);
    }
    R.census = C;
    R.cutLanes = cutLanes;
    r.expect(C.cut > 0, "the phone census found no measured cut preview in the extras fixture");

    // The extras fixture contains a long Harbor output, so the sheet check exercises fetched text that exceeds its preview.
    // The lane is still discovered at runtime, never hardcoded to "harbor".
    const targetId = cutLanes[0];
    r.expect(!!targetId, "no lane had a cut preview to open 'View all' on (census.cut=" + C.cut + ")");
    if (targetId) {
    await goto(page, { v: "session", id: targetId }, D); await page.waitForTimeout(200);
    await page.evaluate(() => { document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click()); document.querySelectorAll('.step > button[aria-expanded="false"]').forEach((x) => x.click()); });
    const btn = page.locator(".viewall:visible").first();
    const btnCount = await btn.count();
    r.expect(btnCount > 0, "no 'View all' button visible on " + targetId + " despite a cut preview in the census");
    if (btnCount > 0) {
      await btn.scrollIntoViewIfNeeded(); await page.evaluate(() => window.scrollBy(0, -250)); await page.waitForTimeout(300);
      const hashBefore = await page.evaluate(() => JSON.stringify(history.state));
      await page.screenshot({ path: path.join(ENV.out, "v-preview.png") });
      const expected = await btn.evaluate((x) => [...x.parentElement.querySelectorAll("pre")].map((q) => q.textContent.length));
      await btn.click(); await page.waitForTimeout(350);
      R.sheet = await page.evaluate(() => { const d = document.querySelector("dialog.panel.full"); const r = d.getBoundingClientRect(); const vw = document.documentElement.clientWidth;
        let out = 0; d.querySelectorAll("*").forEach((e) => { const b = e.getBoundingClientRect(); if (b.width && (b.right > vw + 0.5 || b.left < -0.5)) out++; });
        return { open: d.open, x: r.x, y: r.y, w: r.width, h: r.height, vw, vh: innerHeight, pres: [...d.querySelectorAll("pre")].map((q) => q.textContent.length), sideways: out, bodyScrolls: d.querySelector(".panel-b").scrollHeight > d.querySelector(".panel-b").clientHeight, state: history.state?.sheet ?? 0, focus: document.activeElement?.className }; });
      R.sheet.previewPres = expected;
      await page.screenshot({ path: path.join(ENV.out, "v-sheet.png") });
      await page.evaluate(() => { const b = document.querySelector("dialog.panel.full .panel-b"); b.scrollTop = b.scrollHeight; }); await page.waitForTimeout(200); await page.screenshot({ path: path.join(ENV.out, "v-sheet-end.png") });
      // Back closes it and keeps the page as it was (still expanded, same route).
      await page.goBack(); await page.waitForTimeout(350);
      R.afterBack = await page.evaluate((h) => ({ dialog: !!document.querySelector("dialog.panel.full"), sameState: JSON.stringify(history.state) === h, stepsStillOpen: document.querySelectorAll('.step > button[aria-expanded="true"]').length, htmlLock: document.documentElement.classList.contains("panel-open") }), hashBefore);
      // The close button, then Escape, each restore the same history entry.
      await page.locator(".viewall:visible").first().click(); await page.waitForTimeout(250); await page.click("dialog.panel.full .panel-h .ibtn"); await page.waitForTimeout(350);
      R.afterClose = await page.evaluate((h) => ({ dialog: !!document.querySelector("dialog.panel.full"), sameState: JSON.stringify(history.state) === h, stepsStillOpen: document.querySelectorAll('.step > button[aria-expanded="true"]').length }), hashBefore);
      await page.locator(".viewall:visible").first().click(); await page.waitForTimeout(250); await page.keyboard.press("Escape"); await page.waitForTimeout(350);
      R.afterEsc = await page.evaluate((h) => ({ dialog: !!document.querySelector("dialog.panel.full"), sameState: JSON.stringify(history.state) === h, stepsStillOpen: document.querySelectorAll('.step > button[aria-expanded="true"]').length }), hashBefore);
    }
    }
    R.phoneErrors = page.errors;
    await page.context().close();
  }

  // ---- Desktop, light: the dialog on the sample's harbor lane. ----
  {
    const page = await served(browser, { size: "desktop", dark: false });
    await goto(page, { v: "session", id: "harbor" }, D); await page.waitForTimeout(200);
    await page.evaluate(() => { document.querySelectorAll('.tsum[aria-expanded="false"]').forEach((x) => x.click()); });
    await page.locator(".step > button").first().click(); await page.waitForTimeout(200);
    R.sample = { viewAll: await page.locator(".viewall:visible").count() };
    r.expect(R.sample.viewAll > 0, "no View all button on the sample harbor lane's first expanded step");
    if (R.sample.viewAll) {
      await page.locator(".viewall:visible").first().click(); await page.waitForTimeout(300);
      R.desk = await page.evaluate(() => { const r = document.querySelector("dialog.panel.full").getBoundingClientRect(); return { x: Math.round(r.x), w: Math.round(r.width), h: Math.round(r.height), vh: innerHeight, vw: document.documentElement.clientWidth }; });
      await page.screenshot({ path: path.join(ENV.out, "v-desk.png") });
      await page.mouse.click(40, 40); await page.waitForTimeout(250);
      R.desk.closedByBackdrop = !(await page.$("dialog.panel.full"));
    }
    R.deskErrors = page.errors;
    await page.context().close();
  }

  // ---- Desktop, light and dark: an assistant message's measure with wide mode off (68ch) and on (the column's width). ----
  R.measure = {};
  for (const dark of [false, true]) {
    const page = await served(browser, { size: "desktop", dark });
    const ids = ["harbor", ...Object.keys(D.SESS).filter((id) => id !== "harbor")];
    let found = null;
    for (const id of ids) {
      await goto(page, { v: "session", id }, D); await page.waitForTimeout(100);
      if (await page.locator("#page section[aria-label='Transcript'] .msg.assistant").count()) { found = id; break; }
    }
    // The message's width against a probe that is 68ch in the message's own font, and against the column it sits in.
    const measure = () => page.evaluate(() => {
      const msg = document.querySelector("#page section[aria-label='Transcript'] .msg.assistant");
      const probe = document.createElement("div");
      probe.style.cssText = "position:absolute;visibility:hidden;height:0;width:68ch";
      msg.append(probe);
      const ch68 = probe.getBoundingClientRect().width; probe.remove();
      const cs = getComputedStyle(msg.parentElement), col = msg.parentElement.getBoundingClientRect().width - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
      return { wide: document.querySelector("#page").classList.contains("wide-mode"), w: msg.getBoundingClientRect().width, ch68, col, vw: document.documentElement.clientWidth, sideways: document.documentElement.scrollWidth - document.documentElement.clientWidth };
    });
    const key = dark ? "dark" : "light";
    R.measure[key] = { session: found };
    if (found) {
      R.measure[key].off = await measure();
      await wide(page, true); await page.waitForTimeout(150);
      R.measure[key].on = await measure();
      await page.screenshot({ path: path.join(ENV.out, "wide-session-" + key + ".png") });
    }
    R.measure[key].errors = page.errors;
    await page.context().close();
  }

  r.results = R;
  r.expect((R.phoneErrors ?? []).length === 0, "phone page errors: " + (R.phoneErrors ?? []).join(" | "));
  r.expect((R.deskErrors ?? []).length === 0, "desktop page errors: " + (R.deskErrors ?? []).join(" | "));
  r.expect(R.census?.mismatch === 0, "census mismatch=" + R.census?.mismatch + " " + JSON.stringify(R.census?.bad ?? []));
  r.expect(!!R.sheet, "the 'View all' sheet was never opened, so nothing below could be checked");
  if (R.sheet) {
    r.expect(R.sheet.open === true, "sheet did not open");
    r.expect(R.sheet.sideways === 0, "sheet sideways=" + R.sheet.sideways);
    r.expect(R.sheet.pres.every((len, i) => len >= (R.sheet.previewPres[i] ?? 0)), "View all fetched less than the preview showed");
    // Here the preview is cut on screen only (the sample's output is under the server's 1536-byte preview), so the sheet
    // holds the same text, unclipped. That View all fetches a preview the server cut is asserted in extras.mjs.
  }
  r.expect(!!R.afterBack, "back was never tried on the sheet");
  if (R.afterBack) {
    r.expect(R.afterBack.dialog === false && R.afterBack.sameState && R.afterBack.htmlLock === false, "back did not restore the page: " + JSON.stringify(R.afterBack));
    r.expect(R.afterBack.stepsStillOpen > 0, "steps did not stay open after back");
  }
  r.expect(!!R.afterClose, "the close button was never tried on the sheet");
  if (R.afterClose) r.expect(R.afterClose.dialog === false && R.afterClose.sameState, "close button did not restore the page: " + JSON.stringify(R.afterClose));
  r.expect(!!R.afterEsc, "Escape was never tried on the sheet");
  if (R.afterEsc) r.expect(R.afterEsc.dialog === false && R.afterEsc.sameState, "Escape did not restore the page: " + JSON.stringify(R.afterEsc));
  r.expect(!!R.desk, "the desktop dialog was never opened, so nothing below could be checked");
  if (R.desk) {
    r.expect(R.desk.x >= 0 && R.desk.x + R.desk.w <= R.desk.vw + 0.5, "desktop dialog sideways: " + JSON.stringify(R.desk));
    r.expect(R.desk.closedByBackdrop === true, "backdrop click did not close the desktop dialog");
  }

  for (const key of ["light", "dark"]) {
    const m = R.measure[key];
    r.expect(!!m.session, "no session with an assistant message, so the wide-mode measure (" + key + ") was never checked");
    r.expect(m.errors.length === 0, "wide-mode measure page errors (" + key + "): " + m.errors.join(" | "));
    if (!m.session) continue;
    r.expect(m.off.wide === false && m.on.wide === true, "the wide toggle did not switch the mode (" + key + "): " + JSON.stringify(m));
    r.expect(m.off.w <= m.off.ch68 + 0.5, "with wide mode off an assistant message is wider than 68ch (" + key + "): " + JSON.stringify(m.off));
    r.expect(m.on.w > m.on.ch68 + 1, "with wide mode on an assistant message is still capped at 68ch (" + key + "): " + JSON.stringify(m.on));
    r.expect(m.on.w >= m.on.col - 1 && m.on.w <= m.on.col + 1, "with wide mode on an assistant message does not fill its column (" + key + "): " + JSON.stringify(m.on));
    r.expect(m.on.sideways === 0, "wide mode scrolls sideways (" + key + "): " + JSON.stringify(m.on));
  }

  return r.done();
}

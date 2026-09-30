// A subagent's spawn card keeps its actions row where it is when the "Activity" disclosure opens: Open and the disclosure stay
// side by side at the same top, left and height, the chevron turns in place, and the opened activity sits below the whole row (not
// between Open and the disclosure, and without a gap). Closing it puts every box back. Run at 390x844 and 1280x800, light and dark,
// on the first spawn card that has activity to show; screenshots of the card collapsed and opened go to out/childcard/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, goto, reporter } from "../lib.mjs";

const OUT = path.join(ENV.out, "childcard");
fs.mkdirSync(OUT, { recursive: true });
const near = (a, b) => Math.abs(a - b) <= 1;

// The disclosure reads "Activity", with the count of steps quietly after it (the count "Show all N" uses), and names whose activity it is
// only for assistive technology.
// The boxes the row is made of, rounded to a tenth of a pixel.
const measure = (page) => page.evaluate(() => {
  const c = [...document.querySelectorAll("#page .turns .hcard.child-card")].find((x) => !x.closest(".cw-body") && x.querySelector(".child-actions .cw-toggle"));
  if (!c) return null;
  const r1 = (v) => Math.round(v * 10) / 10, box = (n) => { const b = n.getBoundingClientRect(); return { top: r1(b.top), left: r1(b.left), width: r1(b.width), height: r1(b.height), bottom: r1(b.bottom) }; };
  const open = c.querySelector(".child-actions > button:not(.cw-toggle)"), toggle = c.querySelector(".child-actions .cw-toggle"), body = c.querySelector(".child-actions .cw-body");
  const o = box(open), t = box(toggle), row = { top: Math.min(o.top, t.top), bottom: Math.max(o.bottom, t.bottom) };
  return { open: o, toggle: t, row: { ...row, height: r1(row.bottom - row.top) }, card: box(c), expanded: toggle.getAttribute("aria-expanded"), bodyShown: !!body && !body.hidden && body.getClientRects().length > 0, body: body && !body.hidden ? box(body) : null, label: toggle.textContent.replace(/[\u2009\u00a0]/g, " ").replace(/\s+/g, " ").trim(), aria: toggle.getAttribute("aria-label"), childName: (c.querySelector(".child-head .ln")?.textContent ?? "").replace(/[\u2009\u00a0]/g, " "), showAll: c.querySelector(".child-actions .cw-body .show-all")?.textContent ?? null, chevron: getComputedStyle(toggle.querySelector(".chev")).transform };
});

export default async function childcard(browser) {
  const D = await data();
  const r = reporter("childcard");
  const lanes = Object.keys(D.SESS);
  r.results.runs = [];
  for (const size of ["phone", "desktop"]) {
    for (const dark of [false, true]) {
      const name = size + (dark ? "-dark" : "-light");
      const page = await served(browser, { size, dark });
      let m0 = null;
      for (const id of lanes) {
        await goto(page, { v: "session", id }, D);
        m0 = await measure(page);
        if (m0) break;
      }
      r.expect(!!m0, name + ": the fixture needs a spawn card with activity to show");
      if (!m0) { await page.context().close(); continue; }
      const card = page.locator("#page .turns .hcard.child-card:has(.child-actions .cw-toggle)").first();
      const toggle = card.locator(".child-actions .cw-toggle");
      await card.scrollIntoViewIfNeeded(); await page.waitForTimeout(80);
      const m1 = await measure(page);
      await card.screenshot({ path: path.join(OUT, name + "-collapsed.png") });
      await toggle.click(); await page.waitForTimeout(250);
      const m2 = await measure(page);
      await card.screenshot({ path: path.join(OUT, name + "-open.png") });
      await toggle.click(); await page.waitForTimeout(250);
      const m3 = await measure(page);
      r.results.runs.push({ name, collapsed: m1, open: m2, closed: m3 });
      const at = (m) => name + ": " + JSON.stringify({ open: m.open, toggle: m.toggle, row: m.row });
      r.expect(m1.expanded === "false" && !m1.bodyShown, name + ": collapsed to begin with");
      r.expect(m2.expanded === "true" && m2.bodyShown, name + ": the disclosure opens its activity: " + JSON.stringify({ expanded: m2.expanded, shown: m2.bodyShown }));
      r.expect(/^Activity · \d+$/.test(m1.label) && !/\bdid\b/.test(m1.label), name + ": the disclosure reads \"Activity · N\": " + JSON.stringify(m1.label));
      r.expect(m1.aria === "Activity of " + m1.childName, name + ": its accessible name says whose activity it is: " + JSON.stringify([m1.aria, m1.childName]));
      if (m2.showAll) r.expect(m2.showAll === "Show all " + m1.label.replace(/^Activity · /, ""), name + ": the count is the one Show all uses: " + JSON.stringify([m1.label, m2.showAll]));
      // Open and the disclosure keep their place: top, left and height (within a pixel) and the row keeps its height.
      for (const k of ["open", "toggle"]) {
        r.expect(near(m1[k].top, m2[k].top) && near(m1[k].left, m2[k].left) && near(m1[k].height, m2[k].height), name + ": " + k + " stays put when the disclosure opens: before " + JSON.stringify(m1[k]) + ", after " + JSON.stringify(m2[k]));
      }
      r.expect(near(m1.row.height, m2.row.height), name + ": the row keeps its height: " + m1.row.height + " then " + m2.row.height);
      r.expect(near(m2.open.top, m2.toggle.top) || near(m2.open.top + m2.open.height / 2, m2.toggle.top + m2.toggle.height / 2), name + ": Open and the disclosure share a row when open: " + at(m2));
      r.expect(m2.toggle.left >= m2.open.left + m2.open.width - 1, name + ": the disclosure sits beside Open, to its right: " + at(m2));
      // The activity sits below the whole row, with no gap beyond the card's own spacing.
      r.expect(!!m2.body && m2.body.top >= m2.row.bottom - 1, name + ": the activity starts below the row: body " + JSON.stringify(m2.body) + ", row " + JSON.stringify(m2.row));
      r.expect(!!m2.body && m2.body.top - m2.row.bottom <= 10, name + ": no gap between the row and its activity: " + (m2.body ? m2.body.top - m2.row.bottom : "none"));
      r.expect(near(m1.card.top, m2.card.top), name + ": the card's top does not move: " + m1.card.top + " then " + m2.card.top);
      // The chevron turns in place (a rotation, not a move), and turns back.
      r.expect(m1.chevron !== m2.chevron && m3.chevron === m1.chevron, name + ": the chevron turns and turns back: " + [m1.chevron, m2.chevron, m3.chevron].join(" | "));
      // Closing puts every box back.
      r.expect(m3.expanded === "false" && !m3.bodyShown, name + ": closed again");
      for (const k of ["open", "toggle"]) r.expect(near(m1[k].top, m3[k].top) && near(m1[k].left, m3[k].left) && near(m1[k].height, m3[k].height), name + ": " + k + " is back where it was after closing: " + JSON.stringify(m1[k]) + " then " + JSON.stringify(m3[k]));
      r.expect(near(m1.card.height, m3.card.height), name + ": the card is its old height after closing: " + m1.card.height + " then " + m3.card.height);
      r.expect(page.errors.length === 0, name + ": page errors: " + page.errors.join(" | "));
      await page.context().close();
    }
  }
  return r.done();
}

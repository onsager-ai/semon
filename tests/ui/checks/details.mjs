// The Session details sheet reads as one list: the API-equivalent cost row is a plain row like the ones above it (no button chrome,
// no tap), its breakdown is always shown as a quiet table (kind, tokens, cost) per model, kinds that used and cost nothing are
// left out, and no label wraps at 390 px. Opened on the session with the most cost detail, at 390 and 1280 px, light and dark;
// screenshots go to out/details/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto } from "../lib.mjs";

const OUT = path.join(ENV.out, "details");
fs.mkdirSync(OUT, { recursive: true });

export default async function detailsCheck(browser) {
  const D = await data(), r = reporter("details"), results = {};
  // The session that reports the most cost lines: the richest sheet the fixture can show.
  const sid = Object.values(D.SESS).map((s) => [s, Object.values(s.tokens_by_model ?? {}).length + (s.reported_runs?.length ?? 0)]).sort((a, b) => b[1] - a[1])[0]?.[0]?.id;
  r.expect(!!sid, "the fixture holds no session to open");
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const tag = size + (dark ? "-dark" : "-light"), page = await served(browser, { size, dark });
    await goto(page, { v: "session", id: sid }, D); await page.waitForTimeout(200);
    await page.click("#more-btn"); await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click();
    await page.waitForFunction(() => document.querySelector("dialog.session-details")?.open === true); await page.waitForTimeout(150);
    const m = await page.evaluate(() => {
      const d = document.querySelector("dialog.session-details"), lines = (n) => { const cs = getComputedStyle(n), lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.25; return Math.round(n.getBoundingClientRect().height / lh); };
      const row = d.querySelector(".cost-row"), cs = row && getComputedStyle(row), plain = d.querySelector(".detail-row:not(.cost-row)");
      const labels = [...d.querySelectorAll(".detail-label")].map((n) => ({ text: n.textContent, lines: lines(n) }));
      const groups = [...d.querySelectorAll(".cost-model")].map((g) => {
        const rows = [...g.querySelectorAll(".cost-line")].map((l) => {
          const cells = [...l.children].map((c) => { const range = document.createRange(); range.selectNodeContents(c); const b = c.getBoundingClientRect(), t = range.getBoundingClientRect(); return { text: c.textContent.replace(/\s*\(.*\)$/, ""), tip: c.dataset.tip ?? null, right: Math.round((b.right) * 10) / 10, base: Math.round(t.bottom * 10) / 10, top: Math.round(t.top * 10) / 10, textRight: Math.round(t.right * 10) / 10, size: parseFloat(getComputedStyle(c).fontSize) }; });
          return { cells, tokens: cells[1].text, cost: cells[2].text };
        });
        return { model: g.querySelector(".cost-model-name")?.textContent, rows, head: [...g.querySelector(".cost-model-head").children].map((c) => Math.round(c.getBoundingClientRect().right * 10) / 10) };
      });
      const dr = d.getBoundingClientRect(), vb = d.querySelector(".vb");
      return {
        tag: row?.tagName, background: cs?.backgroundColor, border: cs?.borderTopWidth + "/" + cs?.borderBottomWidth + "/" + cs?.borderLeftWidth, expanded: row?.hasAttribute("aria-expanded"),
        rowSameGrid: !!row && !!plain && cs.display === getComputedStyle(plain).display && cs.paddingTop === getComputedStyle(plain).paddingTop && cs.borderBottomWidth === getComputedStyle(plain).borderBottomWidth,
        labelLeft: row && Math.round(row.querySelector(".detail-label").getBoundingClientRect().left), valueLeft: [...d.querySelectorAll(".detail-row")].map((x) => Math.round(x.querySelector(".detail-value").getBoundingClientRect().left)),
        labels, groups, hidden: !!d.querySelector(".cost-breakdown")?.hidden, breakdownVisible: (d.querySelector(".cost-breakdown")?.getClientRects().length ?? 0) > 0,
        tipIcon: !!d.querySelector(".cost-breakdown-head .cost-info[data-tip]"), sideways: vb.scrollWidth > vb.clientWidth + 1 || document.documentElement.scrollWidth > document.documentElement.clientWidth, inView: dr.left >= 0 && dr.right <= innerWidth + 0.5,
      };
    });
    results[tag] = { tag: m.tag, background: m.background, groups: m.groups.map((g) => g.model + ": " + g.rows.map((x) => x.cells[0].text + " " + x.tokens + " " + x.cost).join(" | ")), labels: m.labels.map((l) => l.text).join(", ") };
    r.expect(m.tag === "DIV" && !m.expanded, tag + ": the cost row is " + m.tag + (m.expanded ? " with aria-expanded" : "") + ", not a plain element");
    r.expect(m.background === "rgba(0, 0, 0, 0)", tag + ": the cost row background is " + m.background + ", not transparent");
    r.expect(m.rowSameGrid, tag + ": the cost row does not share the other rows' grid, padding and hairline");
    r.expect(m.valueLeft.every((x) => x === m.valueLeft[0]), tag + ": values do not start at one x: " + JSON.stringify(m.valueLeft));
    r.expect(m.breakdownVisible && !m.hidden && m.groups.length > 0, tag + ": the breakdown is not shown on open");
    r.expect(m.tipIcon, tag + ": the API-equivalent cost tip is not reachable from the sheet");
    for (const l of m.labels) r.expect(l.lines <= 1, tag + ": the label " + JSON.stringify(l.text) + " wraps onto " + l.lines + " lines");
    r.expect(!m.sideways && m.inView, tag + ": the sheet scrolls sideways or is off screen");
    for (const g of m.groups) {
      r.expect(g.rows.length > 0, tag + ": " + g.model + " lists no rows");
      const edge = (i) => g.rows.map((x) => x.cells[i].right);
      for (const i of [1, 2]) r.expect(new Set([...edge(i), g.head[i]]).size === 1, tag + ": " + g.model + " column " + i + " right edges differ: " + JSON.stringify([...edge(i), g.head[i]]));
      for (const x of g.rows) {
        r.expect(!((x.tokens === "" || x.tokens === "0") && x.cost === "$0.00"), tag + ": " + g.model + " shows a zero row " + x.cells[0].text);
        r.expect(x.cells.every((c) => Math.abs(c.base - x.cells[0].base) <= 1.5), tag + ": " + g.model + " " + x.cells[0].text + " cells do not share a baseline: " + JSON.stringify(x.cells.map((c) => c.base)));
        if (x.tokens) r.expect(/^\d[\d.]*[kM]?$/.test(x.tokens) && /^[\d,]+ (tokens|searches)$/.test(x.cells[1].tip ?? ""), tag + ": " + g.model + " " + x.cells[0].text + " tokens read " + x.tokens + " with tip " + x.cells[1].tip);
      }
    }
    await page.screenshot({ path: path.join(OUT, "details-" + tag + ".png") });
    // The sheet's body may scroll on a phone: a second picture of the bottom, where the notes are.
    await page.evaluate(() => { const vb = document.querySelector("dialog.session-details .vb"); vb.scrollTop = vb.scrollHeight; }); await page.waitForTimeout(100);
    await page.screenshot({ path: path.join(OUT, "details-" + tag + "-end.png") });
    await page.close();
  }
  r.results = results;
  return r.done();
}

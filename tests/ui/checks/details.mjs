// The Session details sheet reads as one list: the API-equivalent cost row is a plain row like the ones above it (no button chrome,
// no tap), its breakdown is always shown as a quiet table (kind, tokens, cost) per model, kinds that used and cost nothing are
// left out, and no label wraps at 390 px. Opened on the session with the most cost detail, and on the extras fixture's web-search
// session (a priced kind with no token count: its tokens cell is empty), at 390 and 1280 px, light and dark; screenshots go to
// out/details/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto } from "../lib.mjs";

const OUT = path.join(ENV.out, "details");
fs.mkdirSync(OUT, { recursive: true });
const near = (a, b) => Math.abs(a - b) <= 1;

// Reads the open sheet. A cell's text is measured on its own text node (the tooltip's screen-reader text sits beside it), and an
// empty cell has none.
const read = (page) => page.evaluate(() => {
  const d = document.querySelector("dialog.session-details"), lines = (n) => { const cs = getComputedStyle(n), lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.25; return Math.round(n.getBoundingClientRect().height / lh); };
  const row = d.querySelector(".cost-row"), cs = row && getComputedStyle(row), plain = d.querySelector(".detail-row:not(.cost-row)");
  const r1 = (v) => Math.round(v * 10) / 10;
  const cell = (c) => {
    const node = c.firstChild, text = node?.nodeType === 3 && node.length ? node : null;
    if (!text) return { text: "", tip: c.dataset.tip ?? null };
    const range = document.createRange(); range.selectNodeContents(text); const t = range.getBoundingClientRect();
    return { text: text.data, tip: c.dataset.tip ?? null, base: r1(t.bottom), textRight: r1(t.right) };
  };
  const groups = [...d.querySelectorAll(".cost-model")].map((g) => ({
    model: g.querySelector(".cost-model-name")?.textContent,
    head: [...g.querySelector(".cost-model-head").children].map(cell),
    rows: [...g.querySelectorAll(".cost-line")].map((l) => { const cells = [...l.children].map(cell); return { cells, kind: cells[0].text, tokens: cells[1].text, cost: cells[2].text }; }),
  }));
  const dr = d.getBoundingClientRect(), vb = d.querySelector(".vb");
  return {
    tag: row?.tagName, background: cs?.backgroundColor, expanded: row?.hasAttribute("aria-expanded"),
    rowSameGrid: !!row && !!plain && cs.display === getComputedStyle(plain).display && cs.paddingTop === getComputedStyle(plain).paddingTop && cs.borderBottomWidth === getComputedStyle(plain).borderBottomWidth,
    valueLeft: [...d.querySelectorAll(".detail-row")].map((x) => Math.round(x.querySelector(".detail-value").getBoundingClientRect().left)),
    labels: [...d.querySelectorAll(".detail-label")].map((n) => ({ text: n.textContent, lines: lines(n) })), groups,
    hidden: !!d.querySelector(".cost-breakdown")?.hidden, breakdownVisible: (d.querySelector(".cost-breakdown")?.getClientRects().length ?? 0) > 0,
    tipIcon: !!d.querySelector(".cost-breakdown-head .cost-info[data-tip]"), sideways: vb.scrollWidth > vb.clientWidth + 1 || document.documentElement.scrollWidth > document.documentElement.clientWidth, inView: dr.left >= 0 && dr.right <= innerWidth + 0.5,
    fonts: { row: parseFloat(getComputedStyle(d.querySelector(".cost-line")).fontSize), detail: parseFloat(getComputedStyle(d.querySelector(".detail-value")).fontSize), name: parseFloat(getComputedStyle(d.querySelector(".cost-model-name")).fontSize) },
  };
});

export default async function detailsCheck(browser) {
  const D = await data(), X = await data({ extras: true }), r = reporter("details"), results = {};
  // The session that reports the most cost lines: the richest sheet the fixture can show.
  const sid = Object.values(D.SESS).map((s) => [s, Object.values(s.tokens_by_model ?? {}).length + (s.reported_runs?.length ?? 0)]).sort((a, b) => b[1] - a[1])[0]?.[0]?.id;
  r.expect(!!sid, "the fixture holds no session to open");
  r.expect(!!X.SESS["web-search"], "the extras fixture holds no web-search session");
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    for (const [name, extras, id] of [["harbor", false, sid], ["web-search", true, "web-search"]]) {
      const tag = size + (dark ? "-dark" : "-light") + " " + name, page = await served(browser, { size, dark, extras });
      await goto(page, { v: "session", id }, extras ? X : D); await page.waitForTimeout(200);
      await page.click("#more-btn"); await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click();
      await page.waitForFunction(() => document.querySelector("dialog.session-details")?.open === true); await page.waitForTimeout(150);
      const m = await read(page);
      results[tag] = { tag: m.tag, background: m.background, fonts: m.fonts, groups: m.groups.map((g) => g.model + ": " + g.rows.map((x) => x.kind + " " + (x.tokens || "-") + " " + x.cost).join(" | ")), labels: m.labels.map((l) => l.text).join(", ") };
      r.expect(m.tag === "DIV" && !m.expanded, tag + ": the cost row is " + m.tag + (m.expanded ? " with aria-expanded" : "") + ", not a plain element");
      r.expect(m.background === "rgba(0, 0, 0, 0)", tag + ": the cost row background is " + m.background + ", not transparent");
      r.expect(m.rowSameGrid, tag + ": the cost row does not share the other rows' grid, padding and hairline");
      r.expect(m.valueLeft.every((x) => x === m.valueLeft[0]), tag + ": values do not start at one x: " + JSON.stringify(m.valueLeft));
      r.expect(m.breakdownVisible && !m.hidden && m.groups.length > 0, tag + ": the breakdown is not shown on open");
      r.expect(m.tipIcon, tag + ": the API-equivalent cost tip is not reachable from the sheet");
      r.expect(m.fonts.row < m.fonts.detail && m.fonts.name <= m.fonts.row, tag + ": the breakdown is not quieter than the detail rows: " + JSON.stringify(m.fonts));
      for (const l of m.labels) r.expect(l.lines <= 1, tag + ": the label " + JSON.stringify(l.text) + " wraps onto " + l.lines + " lines");
      r.expect(!m.sideways && m.inView, tag + ": the sheet scrolls sideways or is off screen");
      for (const g of m.groups) {
        r.expect(g.rows.length > 0, tag + ": " + g.model + " lists no rows");
        for (const x of g.rows) {
          r.expect(!((x.tokens === "" || x.tokens === "0") && x.cost === "$0.00"), tag + ": " + g.model + " shows a zero row " + x.kind);
          // The text of the tokens and cost cells ends where their column head's text ends.
          for (const i of [1, 2]) if (x.cells[i].text) r.expect(near(x.cells[i].textRight, g.head[i].textRight), tag + ": " + g.model + " " + x.kind + " column " + i + " text ends at " + x.cells[i].textRight + ", its head at " + g.head[i].textRight);
          // Cells that have text share a baseline; an empty cell has none to compare.
          const based = x.cells.filter((c) => c.text);
          r.expect(based.every((c) => near(c.base, based[0].base)), tag + ": " + g.model + " " + x.kind + " cells do not share a baseline: " + JSON.stringify(based.map((c) => c.base)));
          if (x.tokens) r.expect(/^\d[\d.]*[kM]?$/.test(x.tokens) && /^[\d,]+ tokens$/.test(x.cells[1].tip ?? ""), tag + ": " + g.model + " " + x.kind + " tokens read " + x.tokens + " with tip " + x.cells[1].tip);
        }
      }
      if (name === "web-search") {
        const rows = m.groups.flatMap((g) => g.rows), web = rows.find((x) => x.kind === "Web search");
        r.expect(!!web && web.tokens === "" && web.cost === "$0.04" && web.cells[1].tip === null, tag + ": the web-search row should show an empty tokens cell and $0.04: " + JSON.stringify(web));
        r.expect(rows.some((x) => x.kind === "Input" && x.tokens === "2k") && rows.some((x) => x.kind === "Output" && x.tokens === "3k"), tag + ": the priced token rows are missing: " + JSON.stringify(rows.map((x) => x.kind + " " + x.tokens)));
        r.expect(!rows.some((x) => x.kind === "Cache read" || x.kind.startsWith("Cache write")), tag + ": a zero cache row is shown");
      }
      const shot = "details-" + (extras ? "web-search-" : "") + size + (dark ? "-dark" : "-light");
      await page.screenshot({ path: path.join(OUT, shot + ".png") });
      // The sheet's body may scroll on a phone: a second picture of the bottom, where the notes are.
      await page.evaluate(() => { const vb = document.querySelector("dialog.session-details .vb"); vb.scrollTop = vb.scrollHeight; }); await page.waitForTimeout(100);
      await page.screenshot({ path: path.join(OUT, shot + "-end.png") });
      await page.close();
    }
  }
  r.results = results;
  return r.done();
}

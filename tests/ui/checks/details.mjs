// The ⋯ menu's cost section reads as one quiet list: its "Tokens by model" table, opened, has a row for each kind of token that was
// used or billed (kinds that used nothing and cost nothing are left out), tokens in compact form with the exact figure as the cell's
// tooltip, amounts ending where their column ends, and no label wraps at 390 px. Opened on the session with the most cost detail, and
// on the extras fixture's web-search session (priced searches only in the total), at 390 and 1280 px, light and dark; screenshots go to
// out/details/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto } from "../lib.mjs";

const OUT = path.join(ENV.out, "details");
fs.mkdirSync(OUT, { recursive: true });

// Reads the open menu with its tokens table shown. A cell's text is on its own text node (the tooltip's screen-reader text sits beside it).
const read = (page) => page.evaluate(() => {
  const d = document.querySelector("dialog.session-menu"), t = d.querySelector(".tokens");
  const lines = (n) => { const cs = getComputedStyle(n), lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.25; return Math.round(n.getBoundingClientRect().height / lh); };
  const cell = (c) => { const node = c.firstChild, text = node?.nodeType === 3 && node.length ? node : null; if (!text) return { text: "", tip: c.dataset.tip ?? null }; const range = document.createRange(); range.selectNodeContents(text); return { text: text.data, tip: c.dataset.tip ?? null, right: Math.round(range.getBoundingClientRect().right * 10) / 10 }; };
  const rows = [...t.querySelectorAll(".tok-line")].map((l) => { const cells = [...l.children].map(cell); return { kind: cells[0].text, tokens: cells[1].text, cost: cells[2].text, cells }; });
  const body = d.querySelector(".panel-b");
  return {
    shown: !t.hidden && t.getClientRects().length > 0, models: [...t.querySelectorAll(".tok-model")].map((x) => x.textContent), rows,
    labels: [...d.querySelectorAll("dl.kv dt")].map((n) => ({ text: n.textContent, lines: lines(n) })),
    sideways: body.scrollWidth > body.clientWidth + 1 || document.documentElement.scrollWidth > document.documentElement.clientWidth,
    inView: (() => { const r = d.getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth + 0.5; })(),
  };
});

export default async function detailsCheck(browser) {
  const D = await data(), X = await data({ extras: true }), r = reporter("details"), results = {};
  const sid = Object.values(D.SESS).map((s) => [s, Object.values(s.tokens_by_model ?? {}).length + (s.reported_runs?.length ?? 0)]).sort((a, b) => b[1] - a[1])[0]?.[0]?.id;
  r.expect(!!sid, "the fixture holds no session to open");
  r.expect(!!X.SESS["web-search"], "the extras fixture holds no web-search session");
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    for (const [name, extras, id] of [["harbor", false, sid], ["web-search", true, "web-search"]]) {
      const tag = size + (dark ? "-dark" : "-light") + " " + name, page = await served(browser, { size, dark, extras });
      await goto(page, { v: "session", id }, extras ? X : D); await page.waitForTimeout(200);
      await page.click("#more-btn"); await page.waitForFunction(() => document.querySelector("dialog.session-menu")?.open === true); await page.waitForTimeout(150);
      await page.click("dialog.session-menu .disclose"); await page.waitForTimeout(150);
      const m = await read(page);
      results[tag] = { models: m.models, rows: m.rows.map((x) => x.kind + " " + (x.tokens || "-") + " " + x.cost) };
      r.expect(m.shown && m.models.length > 0, tag + ": the tokens table is not shown after opening it");
      for (const l of m.labels) r.expect(l.lines <= 1, tag + ": the label " + JSON.stringify(l.text) + " wraps onto " + l.lines + " lines");
      r.expect(!m.sideways && m.inView, tag + ": the menu scrolls sideways or is off screen");
      for (const x of m.rows) {
        r.expect(!((x.tokens === "" || x.tokens === "0") && x.cost === "$0.00"), tag + ": a zero row is shown: " + x.kind);
        if (x.tokens) r.expect(/^\d[\d.]*[kM]?$/.test(x.tokens) && /^[\d,]+ tokens$/.test(x.cells[1].tip ?? ""), tag + ": " + x.kind + " tokens read " + x.tokens + " with tip " + x.cells[1].tip);
        // The amounts share one right edge, and so do the token counts that have text.
      }
      for (const i of [1, 2]) { const edges = m.rows.map((x) => x.cells[i].right).filter((v) => v != null); r.expect(edges.every((v) => Math.abs(v - edges[0]) <= 1), tag + ": column " + i + " does not end at one x: " + JSON.stringify(edges)); }
      if (name === "web-search") r.expect(m.rows.some((x) => x.kind === "Input" && x.tokens === "2k") && m.rows.some((x) => x.kind === "Output" && x.tokens === "3k") && !m.rows.some((x) => x.kind === "Cache read"), tag + ": the priced token rows are wrong: " + JSON.stringify(m.rows.map((x) => x.kind + " " + x.tokens)));
      await page.screenshot({ path: path.join(OUT, "menu-" + (extras ? "web-search-" : "") + size + (dark ? "-dark" : "-light") + ".png") });
      await page.close();
    }
  }
  r.results = results;
  return r.done();
}
